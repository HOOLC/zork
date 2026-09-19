import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { ACCESS_TTL_SEC, REFRESH_RETRY_SEC, SESSION_IDLE_SEC, SESSION_TTL_SEC, bearerToken, denied, digest, limited, nowSeconds, randomSecret, reply, seal, signToken, unseal, validId, verifyToken, type Claims, type Identity, type Tokens } from "./auth";

// These limits apply to an account across every session and endpoint.
export const LIMITS = {
  sessions: 16,
  connections: 8,
  connectsPerMinute: 30,
  requestsPerMinute: 120,
  bytesPerSecond: 4 * 1024 * 1024,
  burstBytes: 8 * 1024 * 1024,
  bytesPerDay: 5 * 1024 * 1024 * 1024,
  frameBytes: 128 * 1024,
  framesPerSecond: 4096,
  burstFrames: 8192,
  framesPerDay: 20_000_000,
};
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
type AccountData = Identity & { blocked: boolean; sessions: Session[] };
type Quota = {
  minute: number;
  requests: number;
  connects: number;
  day: number;
  bytes: number;
  at: number;
  balance: number;
  frameBalance?: number;
  frames?: number;
};
type Connection = { sid: string; exp: number; close: (code: number, reason: string) => void };

/** A single consistency and enforcement boundary for all of one user's relays. */
export class Account extends DurableObject<Env> {
  private connections = new Set<Connection>();

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
  private charge(kind: "request" | "connect" | "bytes", amount = 0): boolean {
    const now = nowSeconds();
    const minute = Math.floor(now / 60),
      day = Math.floor(now / 86400);
    const q = this.ctx.storage.kv.get<Quota>("quota") ?? {
      minute,
      requests: 0,
      connects: 0,
      day,
      bytes: 0,
      at: now,
      balance: LIMITS.burstBytes,
      frameBalance: LIMITS.burstFrames,
      frames: 0,
    };
    if (q.minute !== minute) {
      q.minute = minute;
      q.requests = 0;
      q.connects = 0;
    }
    if (q.day !== day) {
      q.day = day;
      q.bytes = 0;
      q.frames = 0;
    }
    q.frames ??= 0;
    q.frameBalance = Math.min(LIMITS.burstFrames, (q.frameBalance ?? LIMITS.burstFrames) + Math.max(0, now - q.at) * LIMITS.framesPerSecond);
    q.balance = Math.min(LIMITS.burstBytes, q.balance + Math.max(0, now - q.at) * LIMITS.bytesPerSecond);
    q.at = now;
    if (kind === "request") {
      if (q.requests >= LIMITS.requestsPerMinute) return false;
      q.requests++;
    } else if (kind === "connect") {
      if (q.connects >= LIMITS.connectsPerMinute || q.bytes >= LIMITS.bytesPerDay || q.frames >= LIMITS.framesPerDay) return false;
      q.connects++;
    } else {
      // Tiny/empty frames consume CPU and billable events too.
      if (amount > q.balance || q.bytes + amount > LIMITS.bytesPerDay || q.frameBalance < 1 || q.frames >= LIMITS.framesPerDay) return false;
      q.frameBalance--;
      q.frames++;
      q.bytes += amount;
      q.balance -= amount;
    }
    this.ctx.storage.kv.put("quota", q);
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
      if (data.sessions.length >= LIMITS.sessions || !this.charge("request")) return limited();
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
      if (!this.charge("request")) return limited();
      if (session.retry?.hash === hash && session.retry.request === requestId && session.retry.until > nowSeconds()) {
        return reply(await unseal(this.env, session.retry.response));
      }
      if (session.refreshHash !== hash || session.generation !== claims.gen) {
        // A validly signed older refresh credential outside the exact retry is
        // evidence of reuse. Revoke the family, including already-open sockets.
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
    this.save(data);
    for (const connection of [...this.connections]) {
      if (!id || connection.sid === id) connection.close(4001, "session_revoked");
    }
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
    for (const connection of [...this.connections]) {
      if (connection.exp <= now || data.blocked || !data.sessions.some((s) => s.id === connection.sid)) connection.close(4001, "session_expired");
    }
    this.save(data);
  }
  private async schedule(data: AccountData) {
    const deadlines = data.sessions.flatMap((s) => [s.expires, s.idle, ...(s.retry ? [s.retry.until] : [])]).concat([...this.connections].map((c) => c.exp));
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
    if (!this.charge("request")) return limited();
    const path = new URL(request.url).pathname;
    if (path === "/v1/auth/session" && request.method === "GET")
      return reply({
        subject: claims.sub,
        email: data!.email,
        session_id: claims.sid,
        expires_at: claims.exp,
        relay_connections: [...this.connections].filter((c) => c.sid === claims.sid).length,
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
    if (path !== "/relay" || request.method !== "GET") return reply({ error: "not_found" }, 404);
    return this.connectRelay(request, claims);
  }

  private async connectRelay(request: Request, claims: Claims): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return reply({ error: "websocket_required" }, 426);
    if (this.connections.size >= LIMITS.connections || !this.charge("connect")) return limited();
    let server: WebSocket | undefined, upstream: WebSocket | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let dialTimer: ReturnType<typeof setTimeout> | undefined;
    let dialAbort: AbortController | undefined;
    let closed = false;
    const connection: Connection = {
      sid: claims.sid,
      exp: claims.exp,
      close: (code, reason) => {
        if (closed) return;
        closed = true;
        if (timer !== undefined) clearTimeout(timer);
        if (dialTimer !== undefined) clearTimeout(dialTimer);
        dialAbort?.abort();
        for (const socket of [server, upstream]) {
          try {
            socket?.close(code, reason);
          } catch {
            /* already closed */
          }
        }
        this.connections.delete(connection);
      },
    };
    // Reserve before awaiting the container, including in-flight upgrades in
    // the account cap. A concurrent logout can cancel this reservation.
    this.connections.add(connection);
    timer = setTimeout(() => connection.close(4001, "access_expired"), Math.max(0, claims.exp * 1000 - Date.now()));
    try {
      const headers = new Headers({ upgrade: "websocket" });
      const protocol = request.headers.get("sec-websocket-protocol");
      if (protocol) headers.set("sec-websocket-protocol", protocol);
      // Fixed routing and a fresh URL prevent credentials/cookies/query values
      // reaching the relay process, its logs, or an arbitrary backend.
      const dial = new AbortController();
      dialAbort = dial;
      dialTimer = setTimeout(() => dial.abort(), 15_000);
      const response = await this.env.RELAY.getByName("primary").fetch(new Request("http://relay/relay", { headers, signal: dial.signal }));
      clearTimeout(dialTimer);
      dialTimer = undefined;
      dialAbort = undefined;
      upstream = response.webSocket ?? undefined;
      if (!upstream || response.status !== 101) {
        connection.close(1011, "relay_unavailable");
        return reply({ error: "relay_unavailable" }, 502);
      }
      upstream.binaryType = "arraybuffer";
      upstream.accept();
      if (closed || !this.session(this.data(), claims) || claims.exp <= nowSeconds()) {
        upstream.close(4001, "session_revoked");
        connection.close(4001, "session_revoked");
        return denied();
      }
      const pair = new WebSocketPair();
      server = pair[1];
      server.binaryType = "arraybuffer";
      server.accept();
      const forward = (from: WebSocket, to: WebSocket) => {
        from.addEventListener("message", (event) => {
          if (closed) return;
          if (claims.exp <= nowSeconds() || !this.session(this.data(), claims)) {
            connection.close(4001, "session_expired");
            return;
          }
          if (!(event.data instanceof ArrayBuffer) || event.data.byteLength > LIMITS.frameBytes) {
            connection.close(1009, "invalid_frame");
            return;
          }
          if (!this.charge("bytes", event.data.byteLength)) {
            // Closing the account's sockets prevents cycling sessions to bypass
            // a shared byte budget. The persistent budget also gates reconnects.
            for (const c of [...this.connections]) c.close(4008, "account_quota");
            return;
          }
          try {
            to.send(event.data);
          } catch {
            connection.close(1011, "relay_send_failed");
          }
        });
        from.addEventListener("close", () => connection.close(1000, "relay_closed"));
        from.addEventListener("error", () => connection.close(1011, "relay_error"));
      };
      forward(server, upstream);
      forward(upstream, server);
      await this.schedule(this.data()!);
      const selected = response.headers.get("sec-websocket-protocol");
      return new Response(null, {
        status: 101,
        webSocket: pair[0],
        headers: selected ? { "sec-websocket-protocol": selected } : undefined,
      });
    } catch {
      connection.close(1011, "relay_unavailable");
      return reply({ error: "relay_unavailable" }, 502);
    }
  }
}
