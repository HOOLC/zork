import { DurableObject } from "cloudflare:workers";
import { ulid } from "ulid";
import type { Env } from "./env";
import { CODE_TTL_SEC, LOGIN_TTL_SEC, digest, googleIdentity, limited, nowSeconds, randomSecret, readText, reply, seal, unseal, validLoopbackRedirect, validSecret, type Identity, type Tokens } from "./auth";

type Attempt = {
  redirect: string;
  state: string;
  challenge: string;
  name: string;
  browserHash: string;
  nonce: string;
  googleVerifier: string;
  expires: number;
  phase: "waiting" | "started" | "callback" | "complete" | "used" | "cancelled";
  device?: boolean;
  nextPoll?: number;
  receipt?: string;
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

function escape(value: string) {
  return value.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
// A self POST needs a non-null Origin; only same-origin requests receive the
// referrer. Chromium also applies form-action to the subsequent Google redirect.
export function devicePage(title: string, content: string, cookies?: string): Response {
  return new Response(
    `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escape(title)} · Zork</title><style>body{margin:0;background:#f6f5f2;color:#242424;font:16px/1.7 system-ui}main{max-width:420px;margin:15vh auto;padding:32px}h1{font-size:28px}button{font:inherit;background:#242424;color:white;border:0;border-radius:10px;padding:12px 24px;cursor:pointer}p{margin:20px 0}</style><main><p>Zork</p><h1>${escape(title)}</h1>${content}</main></html>`,
    {
      headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "referrer-policy": "same-origin", "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self' https://accounts.google.com; frame-ancestors 'none'", ...(cookies ? { "set-cookie": cookies } : {}) },
    },
  );
}

/** One login attempt, single-use callback and PKCE code, removed on its alarm. */
export class LoginAttempt extends DurableObject<Env> {
  private async google(id: string, attempt: Attempt, browser: string): Promise<Response> {
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
  }
  async startDevice(id: string, challenge: string, name: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      if (this.ctx.storage.kv.get("attempt")) return reply({ error: "login_exists" }, 409);
      if (!validSecret(id) || !validSecret(challenge) || !name.trim() || name.length > 80) return reply({ error: "invalid_login" }, 400);
      const attempt: Attempt = { redirect: "", state: id, challenge, name, browserHash: "", nonce: randomSecret(), googleVerifier: randomSecret(), expires: nowSeconds() + LOGIN_TTL_SEC, phase: "waiting", device: true };
      this.ctx.storage.kv.put("attempt", attempt);
      await this.ctx.storage.setAlarm(attempt.expires * 1000);
      return reply({ verification_uri: `${this.env.PUBLIC_ORIGIN}/v1/auth/device/${id}`, expires_at: attempt.expires, interval: 3 });
    });
  }
  async authorizeDevice(id: string, request: Request): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      if (!attempt?.device || attempt.expires <= nowSeconds() || attempt.phase !== "waiting") return devicePage("登录请求已结束", "<p>请回到 Zork 重新发起登录。</p>");
      if (request.method === "GET") {
        const browser = randomSecret();
        attempt.browserHash = await digest(browser);
        this.ctx.storage.kv.put("attempt", attempt);
        return devicePage(
          "允许设备使用公网连接",
          `<p>正在为 <strong>${escape(attempt.name)}</strong> 登录 Zork。</p><p>账号用于跨网络连接设备，设备间的访问权限仍需通过邀请授权。</p><form method="post"><input type="hidden" name="csrf" value="${browser}"><button>使用 Google 账号继续</button></form>`,
          cookie(this.env.PUBLIC_ORIGIN, id, browser, LOGIN_TTL_SEC),
        );
      }
      const browser =
        (request.headers.get("cookie") ?? "")
          .split(";")
          .map((v) => v.trim())
          .find((v) => v.startsWith(cookieName(this.env.PUBLIC_ORIGIN, id) + "="))
          ?.split("=")[1] ?? "";
      try {
        if (request.headers.get("origin") !== this.env.PUBLIC_ORIGIN || request.headers.get("content-type")?.split(";")[0] !== "application/x-www-form-urlencoded") return reply({ error: "invalid_login_state" }, 400);
        const csrf = new URLSearchParams(await readText(request, 1024)).get("csrf");
        if (!validSecret(browser) || csrf !== browser || (await digest(browser)) !== attempt.browserHash) return reply({ error: "invalid_login_state" }, 400);
      } catch {
        return reply({ error: "invalid_login_state" }, 400);
      }
      attempt.phase = "started";
      this.ctx.storage.kv.put("attempt", attempt);
      return this.google(id, attempt, browser);
    });
  }
  async cancelDevice(verifier: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      if (!attempt?.device || !validSecret(verifier) || (await digest(verifier)) !== attempt.challenge) return reply({ error: "invalid_grant" }, 401);
      if (attempt.receipt && attempt.identity) {
        const tokens = (await unseal(this.env, attempt.receipt)) as { refresh_token: string };
        const result = await this.env.ACCOUNTS.getByName(attempt.identity.sub).logout(tokens.refresh_token, false);
        if (!result.ok) return result;
      }
      attempt.phase = "cancelled";
      delete attempt.receipt;
      this.ctx.storage.kv.put("attempt", attempt);
      return reply({ cancelled: true });
    });
  }
  async pollDevice(verifier: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      if (!attempt?.device || attempt.expires <= nowSeconds() || !validSecret(verifier) || (await digest(verifier)) !== attempt.challenge) return reply({ error: "invalid_grant" }, 401);
      if (attempt.phase === "cancelled") return reply({ error: "login_cancelled" }, 403);
      if (attempt.phase === "used") return attempt.receipt ? reply(await unseal(this.env, attempt.receipt)) : reply({ error: "invalid_grant" }, 401);
      if (attempt.phase !== "complete") {
        if ((attempt.nextPoll ?? 0) > nowSeconds()) return limited(3);
        attempt.nextPoll = nowSeconds() + 3;
        this.ctx.storage.kv.put("attempt", attempt);
        return reply({ pending: true }, 202);
      }
      if (!attempt.identity || !attempt.sessionId) return reply({ error: "invalid_grant" }, 401);
      const result = await this.env.ACCOUNTS.getByName(attempt.identity.sub).create(attempt.identity, attempt.sessionId, attempt.name);
      if (result.ok) {
        const tokens = (await result.json()) as Tokens;
        attempt.receipt = await seal(this.env, tokens);
        attempt.phase = "used";
        attempt.expires = nowSeconds() + CODE_TTL_SEC;
        this.ctx.storage.kv.put("attempt", attempt);
        await this.ctx.storage.setAlarm(attempt.expires * 1000);
        return reply(tokens);
      }
      return result;
    });
  }
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
      return this.google(id, attempt, browser);
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
      const finish = new URL(attempt.device ? `${this.env.PUBLIC_ORIGIN}/v1/auth/device/complete` : attempt.redirect);
      finish.searchParams.set("state", attempt.state);
      const code = url.searchParams.get("code");
      if (url.searchParams.has("error") || !code || code.length > 4096) {
        finish.searchParams.set("error", "login_cancelled");
      } else {
        try {
          attempt.identity = await googleIdentity(this.env, code, attempt.googleVerifier, attempt.nonce);
          const secret = randomSecret();
          if (!attempt.device) attempt.codeHash = await digest(secret);
          attempt.phase = "complete";
          attempt.expires = nowSeconds() + CODE_TTL_SEC;
          attempt.sessionId = ulid();
          // Code is a locator and an independent random capability; no credentials
          // or Google tokens are returned through the browser's URL/history.
          if (!attempt.device) finish.searchParams.set("code", `${id}.${secret}`);
          this.ctx.storage.kv.put("attempt", attempt);
          await this.ctx.storage.setAlarm(attempt.expires * 1000);
        } catch {
          finish.searchParams.set("error", "google_login_failed");
        }
      }
      if (attempt.device) {
        if (attempt.phase !== "complete") {
          attempt.phase = "cancelled";
          this.ctx.storage.kv.put("attempt", attempt);
        }
        finish.searchParams.delete("state");
      }
      return redirect(finish.toString(), cookie(this.env.PUBLIC_ORIGIN, id, "", 0));
    });
  }

  async exchange(secret: string, verifier: string, redirectUri: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const attempt = this.ctx.storage.kv.get<Attempt>("attempt");
      if (
        !attempt ||
        attempt.device ||
        attempt.phase !== "complete" ||
        attempt.expires <= nowSeconds() ||
        !attempt.identity ||
        !attempt.sessionId ||
        !validSecret(secret) ||
        !validSecret(verifier) ||
        (await digest(secret)) !== attempt.codeHash ||
        (await digest(verifier)) !== attempt.challenge ||
        redirectUri !== attempt.redirect
      )
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
  if (!(await consumeLoginRate(env, request))) return limited();
  const id = randomSecret();
  const name = url.searchParams.get("name") ?? "Zork";
  return env.LOGINS.getByName(id).start(id, { redirect: callback, state, challenge, name });
}

export async function consumeLoginRate(env: Env, request: Request): Promise<boolean> {
  const ip = request.headers.get("cf-connecting-ip") ?? "local";
  return env.LOGIN_LIMITS.getByName(await digest(`${env.AUTH_SIGNING_KEY}:${ip}`)).consume();
}
