import { DurableObject } from "cloudflare:workers";
import { ulid } from "ulid";
import type { Env } from "./env";
import { CODE_TTL_SEC, LOGIN_TTL_SEC, digest, googleIdentity, limited, nowSeconds, randomSecret, reply, validLoopbackRedirect, validSecret, type Identity } from "./auth";

type Attempt = {
  redirect: string;
  state: string;
  challenge: string;
  name: string;
  browserHash: string;
  nonce: string;
  googleVerifier: string;
  expires: number;
  phase: "started" | "callback" | "complete" | "used";
  identity?: Identity;
  codeHash?: string;
  sessionId?: string;
};

function cookieName(origin: string, id: string) {
  return `${origin.startsWith("https:") ? "__Host-" : ""}zork_login_${id}`;
}
function cookie(origin: string, id: string, value: string, age: number) {
  return `${cookieName(origin, id)}=${value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${age}${origin.startsWith("https:") ? "; Secure" : ""}`;
}
function redirect(location: string, cookies: string): Response {
  return new Response(null, {
    status: 302,
    headers: {
      location,
      "set-cookie": cookies,
      "cache-control": "no-store",
      "referrer-policy": "no-referrer",
    },
  });
}

/** One login attempt, single-use callback and PKCE code, removed on its alarm. */
export class LoginAttempt extends DurableObject<Env> {
  async start(id: string, params: { redirect: string; state: string; challenge: string; name: string }): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      if (this.ctx.storage.kv.get("attempt")) return reply({ error: "login_exists" }, 409);
      if (!validSecret(id) || !validLoopbackRedirect(params.redirect) || !validSecret(params.state) || !validSecret(params.challenge) || params.name.length > 80) return reply({ error: "invalid_login" }, 400);
      const browser = randomSecret();
      const attempt: Attempt = {
        ...params,
        browserHash: await digest(browser),
        nonce: randomSecret(),
        googleVerifier: randomSecret(),
        expires: nowSeconds() + LOGIN_TTL_SEC,
        phase: "started",
      };
      this.ctx.storage.kv.put("attempt", attempt);
      await this.ctx.storage.setAlarm(attempt.expires * 1000);
      const url = new URL("https://accounts.google.com/o/oauth2/v2/auth");
      for (const [key, value] of Object.entries({
        client_id: this.env.GOOGLE_CLIENT_ID,
        redirect_uri: `${this.env.PUBLIC_ORIGIN}/v1/auth/google/callback`,
        response_type: "code",
        scope: "openid email",
        state: id,
        prompt: "select_account",
        nonce: attempt.nonce,
        code_challenge_method: "S256",
        code_challenge: await digest(attempt.googleVerifier),
      }))
        url.searchParams.set(key, value);
      return redirect(url.toString(), cookie(this.env.PUBLIC_ORIGIN, id, browser, LOGIN_TTL_SEC));
    });
  }

  async callback(id: string, request: Request): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      const cookies = (request.headers.get("cookie") ?? "").split(";").map((value) => value.trim());
      const value = cookies.find((value) => value.startsWith(cookieName(this.env.PUBLIC_ORIGIN, id) + "="))?.split("=")[1] ?? "";
      if (!attempt || attempt.expires <= nowSeconds() || attempt.phase !== "started" || !validSecret(value) || (await digest(value)) !== attempt.browserHash) return reply({ error: "invalid_login_state" }, 400);
      // Consume before any external I/O; a repeated callback cannot replay Google.
      attempt.phase = "callback";
      this.ctx.storage.kv.put("attempt", attempt);
      const url = new URL(request.url);
      const finish = new URL(attempt.redirect);
      finish.searchParams.set("state", attempt.state);
      const code = url.searchParams.get("code");
      if (url.searchParams.has("error") || !code || code.length > 4096) {
        finish.searchParams.set("error", "login_cancelled");
      } else {
        try {
          attempt.identity = await googleIdentity(this.env, code, attempt.googleVerifier, attempt.nonce);
          const secret = randomSecret();
          attempt.codeHash = await digest(secret);
          attempt.phase = "complete";
          attempt.expires = nowSeconds() + CODE_TTL_SEC;
          attempt.sessionId = ulid();
          // Code is a locator and an independent random capability; no credentials
          // or Google tokens are returned through the browser's URL/history.
          finish.searchParams.set("code", `${id}.${secret}`);
          this.ctx.storage.kv.put("attempt", attempt);
          await this.ctx.storage.setAlarm(attempt.expires * 1000);
        } catch {
          finish.searchParams.set("error", "google_login_failed");
        }
      }
      return redirect(finish.toString(), cookie(this.env.PUBLIC_ORIGIN, id, "", 0));
    });
  }

  async exchange(secret: string, verifier: string, redirectUri: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      if (!attempt || attempt.phase !== "complete" || attempt.expires <= nowSeconds() || !attempt.identity || !attempt.sessionId || !validSecret(secret) || !validSecret(verifier) || (await digest(secret)) !== attempt.codeHash || (await digest(verifier)) !== attempt.challenge || redirectUri !== attempt.redirect)
        return reply({ error: "invalid_grant" }, 401);
      attempt.phase = "used";
      this.ctx.storage.kv.put("attempt", attempt);
      return this.env.ACCOUNTS.getByName(attempt.identity.sub).create(attempt.identity, attempt.sessionId, attempt.name);
    });
  }

  async alarm() {
    await this.ctx.storage.deleteAll();
  }
}

/** Only login creation reaches this object, keyed by a salted address hash. */
export class LoginLimiter extends DurableObject<Env> {
  async consume(): Promise<boolean> {
    const now = nowSeconds();
    const minute = Math.floor(now / 60);
    const day = Math.floor(now / 86400);
    const old = this.ctx.storage.kv.get<{
      minute: number;
      short: number;
      day: number;
      daily: number;
    }>("limits");
    const next = {
      minute,
      day,
      short: old?.minute === minute ? old.short : 0,
      daily: old?.day === day ? old.daily : 0,
    };
    if (next.short >= 10 || next.daily >= 100) return false;
    next.short++;
    next.daily++;
    this.ctx.storage.kv.put("limits", next);
    await this.ctx.storage.setAlarm((day + 1) * 86400 * 1000);
    return true;
  }
  async alarm() {
    await this.ctx.storage.deleteAll();
  }
}

export async function googleStart(env: Env, request: Request): Promise<Response> {
  const url = new URL(request.url);
  const state = url.searchParams.get("state") ?? "";
  const challenge = url.searchParams.get("code_challenge") ?? "";
  const callback = url.searchParams.get("redirect_uri") ?? "";
  if (!validLoopbackRedirect(callback) || !validSecret(state) || !validSecret(challenge) || url.searchParams.get("code_challenge_method") !== "S256") return reply({ error: "invalid_login" }, 400);
  const ip = request.headers.get("cf-connecting-ip") ?? "local";
  if (!(await env.LOGIN_LIMITS.getByName(await digest(`${env.AUTH_SIGNING_KEY}:${ip}`)).consume())) return limited();
  const id = randomSecret();
  const name = url.searchParams.get("name") ?? "Zork";
  return env.LOGINS.getByName(id).start(id, { redirect: callback, state, challenge, name });
}
