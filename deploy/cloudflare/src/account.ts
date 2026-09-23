import { decodeKey } from "./pkarr";
import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { ACCESS_TTL_SEC, REFRESH_RETRY_SEC, SESSION_IDLE_SEC, SESSION_TTL_SEC, bearerToken, denied, digest, limited, nowSeconds, randomSecret, readJson, reply, seal, signToken, unseal, validId, verifyToken, type Claims, type Identity, type Tokens } from "./auth";

const LIMITS = { sessions: 16, requestsPerMinute: 120 };
type Retry = { hash: string; request: string; until: number; response: string };
type Session = {
  id: string;
  name: string;
  created: number;
  expires: number;
  idle: number;
  generation: number;
  refreshHash: string;
  retry?: Retry;
};
type Device = {
  origin: string;
  name: string;
  station: boolean;
  address: unknown;
  channel: string;
  session: string;
};
type AccountData = Identity & { blocked: boolean; sessions: Session[]; devices?: Device[] };
/** Optional cloud account sessions do not authorize Mesh peers or transport. */
export class Account extends DurableObject<Env> {
  private data(): AccountData | undefined {
    return this.ctx.storage.kv.get<AccountData>("account");
  }
  private save(data: AccountData) {
    this.ctx.storage.kv.put("account", data);
  }
  private session(data: AccountData | undefined, claims: Claims): Session | undefined {
    const now = nowSeconds();
    return data && !data.blocked && data.sub === claims.sub ? data.sessions.find((s) => s.id === claims.sid && s.expires > now && s.idle > now) : undefined;
  }
  private charge(): boolean {
    const minute = Math.floor(nowSeconds() / 60);
    const old = this.ctx.storage.kv.get<{ minute: number; requests: number }>("quota");
    const quota = old?.minute === minute ? old : { minute, requests: 0 };
    if (quota.requests >= LIMITS.requestsPerMinute) return false;
    quota.requests++;
    this.ctx.storage.kv.put("quota", quota);
    return true;
  }

  private async mint(data: AccountData, session: Session): Promise<Tokens> {
    const now = nowSeconds();
    const expires = Math.min(now + ACCESS_TTL_SEC, session.expires, session.idle);
    const claims = { sub: data.sub, email: data.email, sid: session.id };
    const access = await signToken(this.env, "access", claims, expires, now);
    const refresh = await signToken(
      this.env,
      "refresh",
      {
        ...claims,
        gen: session.generation,
        nonce: randomSecret(),
      },
      session.expires,
      now,
    );
    return {
      access_token: access,
      refresh_token: refresh,
      token_type: "Bearer",
      subject: data.sub,
      email: data.email,
      session_id: session.id,
      expires_at: expires,
      session_expires_at: session.expires,
      refresh_expires_at: session.idle,
    };
  }

  async create(identity: Identity, id: string, name: string): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      const now = nowSeconds();
      const data = this.data() ?? { ...identity, blocked: false, sessions: [] };
      if (data.sub !== identity.sub || !validId(id) || name.length > 80) return denied();
      if (data.blocked) return reply({ error: "account_blocked" }, 403);
      this.prune(data);
      if (data.sessions.length >= LIMITS.sessions || !this.charge()) return limited();
      data.email = identity.email;
      const session: Session = {
        id,
        name,
        created: now,
        expires: now + SESSION_TTL_SEC,
        idle: now + SESSION_IDLE_SEC,
        generation: 0,
        refreshHash: "",
      };
      const tokens = await this.mint(data, session);
      session.refreshHash = await digest(tokens.refresh_token);
      data.sessions.push(session);
      this.save(data);
      await this.schedule(data);
      return reply(tokens);
    });
  }

  async refresh(token: string, requestId: string): Promise<Response> {
    const claims = await verifyToken(this.env, token, "refresh");
    if (!claims || !validId(requestId)) return denied();
    const hash = await digest(token);
    return this.ctx.blockConcurrencyWhile(async () => {
      const data = this.data();
      const session = this.session(data, claims);
      if (!session || !data) return denied();
      if (!this.charge()) return limited();
      if (session.retry?.hash === hash && session.retry.request === requestId && session.retry.until > nowSeconds()) {
        return reply(await unseal(this.env, session.retry.response));
      }
      if (session.refreshHash !== hash || session.generation !== claims.gen) {
        // A validly signed older refresh credential outside the exact retry is
        // evidence of reuse. Revoke the family, including its access credentials.
        this.revoke(data, session.id);
        return reply({ error: "refresh_reused" }, 401);
      }
      session.generation++;
      session.idle = Math.min(session.expires, nowSeconds() + SESSION_IDLE_SEC);
      const tokens = await this.mint(data, session);
      session.retry = {
        hash,
        request: requestId,
        until: nowSeconds() + REFRESH_RETRY_SEC,
        response: await seal(this.env, tokens),
      };
      session.refreshHash = await digest(tokens.refresh_token);
      this.save(data);
      await this.schedule(data);
      return reply(tokens);
    });
  }

  private revoke(data: AccountData, id?: string) {
    data.sessions = id ? data.sessions.filter((s) => s.id !== id) : [];
    data.devices = data.devices?.filter((device) => data.sessions.some((session) => session.id === device.session));
    this.save(data);
  }

  async logout(token: string, all: boolean): Promise<Response> {
    const claims = await verifyToken(this.env, token, "refresh");
    if (!claims) return denied();
    // Even a previously rotated credential may revoke its own session; it may
    // revoke other sessions only while it is the current refresh credential.
    const hash = await digest(token);
    const data = this.data();
    if (!data || data.sub !== claims.sub) return denied();
    const session = this.session(data, claims);
    if (all && (!session || session.refreshHash !== hash)) return denied();
    this.revoke(data, all ? undefined : claims.sid);
    await this.schedule(data);
    return reply({ revoked: true, scope: all ? "account" : "session" });
  }

  async administer(blocked: boolean): Promise<Response> {
    const data = this.data();
    if (!data) return reply({ error: "account_not_found" }, 404);
    data.blocked = blocked;
    if (blocked) this.revoke(data);
    else this.save(data);
    await this.schedule(data);
    return reply({ blocked: data.blocked });
  }

  private prune(data: AccountData) {
    const now = nowSeconds();
    data.sessions = data.sessions.filter((s) => s.expires > now && s.idle > now);
    for (const session of data.sessions) if (session.retry && session.retry.until <= now) delete session.retry;
    this.save(data);
  }
  private async schedule(data: AccountData) {
    const deadlines = data.sessions.flatMap((s) => [s.expires, s.idle, ...(s.retry ? [s.retry.until] : [])]);
    if (deadlines.length) await this.ctx.storage.setAlarm(Math.max(Date.now() + 1000, Math.min(...deadlines) * 1000));
    else await this.ctx.storage.deleteAlarm();
  }
  async alarm() {
    const data = this.data();
    if (data) {
      this.prune(data);
      await this.schedule(data);
    }
  }

  async fetch(request: Request): Promise<Response> {
    const token = bearerToken(request);
    const claims = token ? await verifyToken(this.env, token, "access") : null;
    const data = this.data();
    if (!claims || !this.session(data, claims)) return denied();
    if (!this.charge()) return limited();
    const path = new URL(request.url).pathname;
    if (path === "/v1/auth/devices" && request.method === "PUT") {
      try {
        const body = await readJson(request);
        if (
          typeof body.origin !== "string" ||
          !/^key:[a-z0-9]{52}$/.test(body.origin) ||
          typeof body.name !== "string" ||
          body.name.length > 128 ||
          typeof body.station !== "boolean" ||
          !["release", "dev", "test"].includes(String(body.channel)) ||
          typeof body.signature !== "string" ||
          !/^[a-f0-9]{128}$/.test(body.signature) ||
          !body.address ||
          JSON.stringify(body.address).length > 4096
        )
          return reply({ error: "invalid_device" }, 400);
        const key = await crypto.subtle.importKey("raw", decodeKey(body.origin.slice(4)), { name: "Ed25519" }, false, ["verify"]);
        const signature = Uint8Array.from(body.signature.match(/../g)!, (byte) => parseInt(byte, 16));
        const message = new TextEncoder().encode(`zork-account-device-v1:${claims.sid}:${body.origin}`);
        if (!(await crypto.subtle.verify("Ed25519", key, signature, message))) return denied();
        // Re-read after verification; logout/revocation may have run while awaiting crypto.
        const current = this.data();
        if (!current || !this.session(current, claims)) return denied();
        const devices = (current.devices ?? []).filter((device) => current.sessions.some((session) => session.id === device.session && session.expires > nowSeconds() && session.idle > nowSeconds()));
        const index = devices.findIndex((device) => device.origin === body.origin);
        if (index < 0 && devices.length >= 64) return limited();
        const device: Device = {
          origin: body.origin,
          name: body.name,
          station: body.station,
          address: body.address,
          channel: String(body.channel),
          session: claims.sid,
        };
        if (index < 0) devices.push(device);
        else devices[index] = device;
        current.devices = devices;
        this.save(current);
        return reply({
          devices: devices.filter((item) => item.channel === body.channel && item.origin !== body.origin).map(({ session: _, ...item }) => item),
        });
      } catch {
        return reply({ error: "invalid_device" }, 400);
      }
    }
    if (path === "/v1/auth/session" && request.method === "GET")
      return reply({
        subject: claims.sub,
        email: data!.email,
        session_id: claims.sid,
        expires_at: claims.exp,
      });
    if (path === "/v1/auth/sessions" && request.method === "GET")
      return reply({
        sessions: data!.sessions
          .filter((s) => s.expires > nowSeconds() && s.idle > nowSeconds())
          .map((s) => ({
            id: s.id,
            name: s.name,
            created_at: s.created,
            expires_at: s.expires,
            refresh_expires_at: s.idle,
            current: s.id === claims.sid,
          })),
      });
    const target = /^\/v1\/auth\/sessions\/([0-7][0-9A-HJKMNP-TV-Z]{25})$/.exec(path)?.[1];
    if (target && request.method === "DELETE") {
      this.revoke(data!, target);
      await this.schedule(data!);
      return reply({ revoked: true });
    }
    return reply({ error: "not_found" }, 404);
  }
}
