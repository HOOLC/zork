import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { limited, nowSeconds, reply } from "./auth";
import { CLIENT_AUTH_HEADER, PROTOCOL_NAMES, STATUS, encodeChallenge, encodeConfirm, encodeDeny, encodeEndpointGone, encodePong, encodeRestarting, encodeStatus, headerClaim, hex, negotiate, parseClientFrame, relayDatagram, unhex, verifyClientAuth, type ProtocolVersion } from "./relay-protocol";

// The public relay: one Durable Object that speaks the iroh relay protocol
// (iroh-relay 1.1, see relay-protocol.ts) directly over hibernatable WebSockets.
// No container and no upstream socket: between messages the object can be
// evicted from memory while Cloudflare keeps the client sockets open, so it is
// billed per message rather than per second of wall time.
//
// Everything a message needs survives eviction:
//   - socket tags (fixed at accept): the client IP budget key, and the EndpointId
//     the client named in its (unverifiable) auth header, or "anon";
//   - socket attachments: protocol version, pending challenge, the verified
//     EndpointId, connection order (newest connection of an EndpointId is the
//     active one) and the peers it has sent to (for EndpointGone);
//   - SQLite storage: budget counters, written sparingly (see Budgets below).
// In-memory maps are caches rebuilt from those on demand.
//
// Relay admission is a cost bound, never a Mesh membership authority.
// Budgets are kept per client key so one noisy client (or a household of
// devices behind one NAT) cannot lock everybody else out:
//   - "ip:<addr>"   every upgrade, keyed by CF-Connecting-IP (IPv6 by /64 so a
//                   host cannot rotate through its own prefix);
//   - "ep:<hex>"    the iroh EndpointId, applied once the client has signed our
//                   challenge. The EndpointId in `x-iroh-relay-client-auth-v1`
//                   signs TLS keying material of the client<->Cloudflare session
//                   and cannot be verified here; EndpointIds are public, so it is
//                   never used for admission, only as a routing tag that must
//                   match the proven key;
//   - "global"      a service-wide safety cap sized to operating cost.
//
// Global cap, justified against Cloudflare pricing (Workers Paid, 2026):
//   - frames: every inbound WebSocket message to a Durable Object is billed as a
//     request at 20:1 (outbound messages are free). 40M frames/day = 2M billed
//     requests/day ≈ 60M/month ≈ $9 at $0.15/M. That is the dominant variable
//     cost, so it is the tightest cap. An idle iroh client pings every 15 s
//     (~5.8k frames/day), so the cap also bounds how many idle clients we pay for.
//   - bytes: Workers do not bill egress; the byte cap bounds abuse volume.
//   - connections: one object handles every socket on one thread; 512 sockets
//     ≈ 150-250 online Zork processes keeps per-message CPU far from the limit.
//   - duration: with hibernation ≈ 0 when idle; while messages arrive more than
//     every ~10 s the object stays resident, at most one 128 MB object
//     (≈ 324k GB-s/month, inside the Workers Paid included 400k GB-s).
//   - storage: SQLite rows written are billed ($1/M beyond 50M/month), so
//     counters live in memory and are written only when they moved by a
//     meaningful amount (FLUSH_BYTES/FLUSH_FRAMES) or at the periodic sweep.
export type Budget = {
  connections: number;
  connectsPerMinute: number;
  bytesPerSecond: number;
  burstBytes: number;
  bytesPerDay: number;
  framesPerSecond: number;
  burstFrames: number;
  framesPerDay: number;
};
const MiB = 1024 * 1024,
  GiB = 1024 * MiB;
export const LIMITS = {
  global: {
    connections: 512,
    connectsPerMinute: 600,
    bytesPerSecond: 32 * MiB,
    burstBytes: 64 * MiB,
    bytesPerDay: 20 * GiB,
    framesPerSecond: 16384,
    burstFrames: 32768,
    framesPerDay: 40_000_000,
  },
  // Tens of connections: a home/office NAT with several desktops, Stations and
  // phones. Per-IP daily volume is a fifth of the global budget.
  ip: {
    connections: 32,
    connectsPerMinute: 60,
    bytesPerSecond: 4 * MiB,
    burstBytes: 8 * MiB,
    bytesPerDay: 4 * GiB,
    framesPerSecond: 4096,
    burstFrames: 8192,
    framesPerDay: 8_000_000,
  },
  // One device identity: its connection plus reconnect overlap (a displaced
  // connection stays open as inactive until it closes, as in iroh-relay).
  endpoint: {
    connections: 4,
    connectsPerMinute: 20,
    bytesPerSecond: 4 * MiB,
    burstBytes: 8 * MiB,
    bytesPerDay: 2 * GiB,
    framesPerSecond: 4096,
    burstFrames: 8192,
    framesPerDay: 4_000_000,
  },
  // A relay datagram frame is at most 1 + 32 + 3 + 64 KiB; anything larger is
  // refused before parsing.
  frameBytes: 128 * 1024,
  // A connection must finish the signed challenge in this time, so an
  // unauthenticated socket cannot sit on IP capacity without an identity.
  authSeconds: 30,
} satisfies Record<"global" | "ip" | "endpoint", Budget> & Record<string, unknown>;

// Counters that moved by at least this much since their last write are
// persisted within FLUSH_SOON_SECONDS (before the object can hibernate, which
// needs ~10 s without events); smaller movements wait for the periodic sweep.
// An eviction therefore forgets less than this per key.
const FLUSH_BYTES = 1 * MiB;
const FLUSH_FRAMES = 1000;
const FLUSH_SECONDS = 30;
const FLUSH_SOON_SECONDS = 5;
// Periodic sweep while any socket is open: dead-socket cleanup and a full
// counter flush. iroh clients ping every 15 s (after their last received
// message), so a socket silent for STALE_SECONDS is gone.
const SWEEP_SECONDS = 300;
const STALE_SECONDS = 300;
// Retry-After for concurrency rejections: long enough to stop a reconnect
// storm, short enough that a reconnecting device recovers quickly.
const BUSY_RETRY_SECONDS = 30;
// iroh-relay's per-endpoint sent_to set, bounded to fit a socket attachment.
const SENT_TO_LIMIT = 20;
const GLOBAL = "global";
const ANON = "anon";
const OPEN = 1;

type Quota = {
  minute: number;
  connects: number;
  day: number;
  bytes: number;
  frames: number;
  at: number;
  balance: number;
  frameBalance: number;
  // Totals at the last write, to decide whether a write is worth it.
  savedBytes: number;
  savedFrames: number;
};
type Denial = { key: string; retryAfter: number };

/** Per-socket state; survives hibernation (serializeAttachment, ≤ 2 KiB). */
export type Attachment = {
  ip: string;
  version: ProtocolVersion;
  since: number;
  /** Pending challenge (hex) until the client authenticated. */
  challenge?: string;
  /** Verified EndpointId (hex). */
  ep?: string;
  /** Authentication order; the highest of an EndpointId is its active connection. */
  seq?: number;
  /** RateLimited status already sent on this connection. */
  limited?: boolean;
  /** EndpointIds this connection delivered datagrams to (hex). */
  sentTo?: string[];
};

export function budgetFor(key: string): Budget {
  return key === GLOBAL ? LIMITS.global : key.startsWith("ep:") ? LIMITS.endpoint : LIMITS.ip;
}
const storageKey = (key: string) => "quota:" + key;

/** Client IP budget key; IPv6 addresses share their /64. */
export function ipKey(request: Request): string {
  const raw = request.headers.get("cf-connecting-ip")?.trim() ?? "";
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(raw)) return "ip:" + raw;
  if (raw.includes(":") && /^[0-9a-fA-F:.]+$/.test(raw)) {
    const [head, tail = ""] = raw.toLowerCase().split("::");
    const left = head ? head.split(":") : [];
    const right = raw.includes("::") && tail ? tail.split(":") : [];
    const groups = raw.includes("::") ? [...left, ...Array(Math.max(0, 8 - left.length - right.length)).fill("0"), ...right] : left;
    return (
      "ip:" +
      groups
        .slice(0, 4)
        .map((g) => g.replace(/^0+(?=.)/, ""))
        .join(":") +
      "::/64"
    );
  }
  // Without Cloudflare's client address (local tests), all clients share one key.
  return "ip:unknown";
}

function secondsUntilTomorrow(now: number) {
  return 86400 - (now % 86400);
}

export class RelayHub extends DurableObject<Env> {
  // ---- caches (rebuilt after eviction) ----
  protected quotas = new Map<string, Quota>();
  protected dirty = new Set<string>();
  private flushAt = 0;
  private alarmAt: number | null | undefined; // undefined: unknown after eviction
  private attachments = new WeakMap<WebSocket, Attachment>();
  private active = new Map<string, WebSocket>();
  private lastSeen = new WeakMap<WebSocket, number>();
  private verifying = new WeakSet<WebSocket>();
  private departed = new WeakSet<WebSocket>();
  private counter = 0;
  private sweptDay = -1;

  // ---- budgets ----

  protected quota(key: string, now = nowSeconds()): Quota {
    const limits = budgetFor(key);
    const minute = Math.floor(now / 60),
      day = Math.floor(now / 86400);
    let q = this.quotas.get(key);
    if (!q) {
      q = this.ctx.storage.kv.get<Quota>(storageKey(key)) ?? {
        minute,
        connects: 0,
        day,
        bytes: 0,
        frames: 0,
        at: now,
        balance: limits.burstBytes,
        frameBalance: limits.burstFrames,
        savedBytes: 0,
        savedFrames: 0,
      };
      this.quotas.set(key, q);
    }
    if (q.minute !== minute) {
      q.minute = minute;
      q.connects = 0;
    }
    if (q.day !== day) {
      q.day = day;
      q.bytes = q.frames = q.savedBytes = q.savedFrames = 0;
    }
    const elapsed = Math.max(0, now - q.at);
    q.frameBalance = Math.min(limits.burstFrames, q.frameBalance + elapsed * limits.framesPerSecond);
    q.balance = Math.min(limits.burstBytes, q.balance + elapsed * limits.bytesPerSecond);
    q.at = now;
    return q;
  }

  protected store(key: string, q: Quota) {
    q.savedBytes = q.bytes;
    q.savedFrames = q.frames;
    this.quotas.set(key, q);
    this.dirty.delete(key);
    this.ctx.storage.kv.put(storageKey(key), q);
  }

  private significant(key: string) {
    const q = this.quotas.get(key);
    return !!q && (q.bytes - q.savedBytes >= FLUSH_BYTES || q.frames - q.savedFrames >= FLUSH_FRAMES);
  }

  /** Writes dirty counters: significant ones, or all of them. */
  private flush(now: number, all: boolean) {
    for (const key of [...this.dirty]) {
      const q = this.quotas.get(key);
      if (!q) this.dirty.delete(key);
      else if (all || this.significant(key)) this.store(key, q);
    }
    this.flushAt = now + FLUSH_SECONDS;
  }

  /** Charges a connect to every key, all or nothing. */
  private admit(keys: string[], now: number, live: (key: string) => number): Denial | null {
    const quotas = keys.map((key) => [key, this.quota(key, now)] as const);
    for (const [key, q] of quotas) {
      const limits = budgetFor(key);
      if (live(key) >= limits.connections) return { key, retryAfter: BUSY_RETRY_SECONDS };
      if (q.connects >= limits.connectsPerMinute) return { key, retryAfter: 60 - (now % 60) };
      if (q.bytes >= limits.bytesPerDay || q.frames >= limits.framesPerDay) return { key, retryAfter: secondsUntilTomorrow(now) };
    }
    for (const [key, q] of quotas) {
      q.connects++;
      this.dirty.add(key);
    }
    return null;
  }

  /**
   * Charges one inbound message to every key, all or nothing. Returns the key
   * that refused it: `daily` when its day is used up, otherwise its per-second
   * rate bucket is momentarily empty.
   */
  private charge(keys: string[], amount: number, now: number): { key: string; daily: boolean } | null {
    const quotas = keys.map((key) => [key, this.quota(key, now)] as const);
    for (const [key, q] of quotas) {
      const limits = budgetFor(key);
      // Tiny/empty frames consume CPU and billable events too.
      if (q.bytes + amount > limits.bytesPerDay || q.frames >= limits.framesPerDay) {
        // Mark the day used up so reconnects are refused at admission too.
        q.bytes = Math.max(q.bytes, limits.bytesPerDay);
        this.store(key, q);
        return { key, daily: true };
      }
      if (amount > q.balance || q.frameBalance < 1) return { key, daily: false };
    }
    for (const [key, q] of quotas) {
      q.frameBalance -= 1;
      q.frames += 1;
      q.bytes += amount;
      q.balance -= amount;
      this.dirty.add(key);
    }
    return null;
  }

  /** After charging: persist significant movement now or before hibernation can drop it. */
  private async settle(now: number) {
    if (now >= this.flushAt) this.flush(now, false);
    for (const key of this.dirty) {
      if (this.significant(key)) {
        await this.ensureAlarm(now + FLUSH_SOON_SECONDS);
        break;
      }
    }
  }

  // ---- sockets ----

  protected attachment(ws: WebSocket): Attachment | null {
    let att = this.attachments.get(ws);
    if (!att) {
      att = (ws.deserializeAttachment() as Attachment | null) ?? undefined;
      if (att) this.attachments.set(ws, att);
    }
    return att ?? null;
  }
  private save(ws: WebSocket, att: Attachment) {
    this.attachments.set(ws, att);
    ws.serializeAttachment(att);
  }
  private open(tag?: string): WebSocket[] {
    // getWebSockets(undefined) is not getWebSockets(): pass no argument for "all".
    return (tag === undefined ? this.ctx.getWebSockets() : this.ctx.getWebSockets(tag)).filter((ws) => ws.readyState === OPEN);
  }
  /** Authenticated open connections of an EndpointId, active (newest) last. */
  protected connections(ep: string): Array<[WebSocket, Attachment]> {
    const found: Array<[WebSocket, Attachment]> = [];
    for (const ws of [...this.open("ep:" + ep), ...this.open(ANON)]) {
      const att = this.attachment(ws);
      if (att?.ep === ep && !this.departed.has(ws)) found.push([ws, att]);
    }
    return found.sort((a, b) => a[1].seq! - b[1].seq!);
  }
  private activeFor(ep: string): WebSocket | undefined {
    const cached = this.active.get(ep);
    if (cached && cached.readyState === OPEN && !this.departed.has(cached)) return cached;
    const newest = this.connections(ep).at(-1)?.[0];
    if (newest) this.active.set(ep, newest);
    else this.active.delete(ep);
    return newest;
  }
  private send(ws: WebSocket, frame: Uint8Array | null) {
    if (!frame) return true;
    try {
      ws.send(frame);
      return true;
    } catch {
      return false;
    }
  }
  private closeSocket(ws: WebSocket, code: number, reason: string) {
    try {
      ws.close(code, reason);
    } catch {
      /* already closed */
    }
    this.gone(ws);
  }
  private nextSeq() {
    // Monotonic across evictions: wall clock milliseconds plus a tie breaker.
    return Date.now() * 1000 + (this.counter++ % 1000);
  }

  private async ensureAlarm(at: number) {
    if (this.alarmAt === undefined) {
      const current = await this.ctx.storage.getAlarm();
      this.alarmAt = current === null ? null : Math.floor(current / 1000);
    }
    if (this.alarmAt === null || this.alarmAt > at) {
      await this.ctx.storage.setAlarm(at * 1000);
      this.alarmAt = at;
    }
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return reply({ error: "websocket_required" }, 426);
    const version = negotiate(request.headers.get("sec-websocket-protocol"));
    if (!version) return reply({ error: "unsupported_relay_protocol", supported: "iroh-relay-v2, iroh-relay-v1" }, 400);
    const now = nowSeconds();
    this.sweepHandshakes(now);
    const ip = ipKey(request);
    const denial = this.admit([GLOBAL, ip], now, (key) => (key === GLOBAL ? this.open() : this.open(key)).length);
    if (denial) return limited(denial.retryAfter);
    const claim = headerClaim(request.headers.get(CLIENT_AUTH_HEADER));
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    this.ctx.acceptWebSocket(server, [ip, claim ? "ep:" + hex(claim) : ANON]);
    const challenge = crypto.getRandomValues(new Uint8Array(16));
    this.save(server, { ip, version, since: now, challenge: hex(challenge) });
    this.lastSeen.set(server, now);
    // The header's key-material signature cannot be checked behind Cloudflare,
    // so every client proves its key with the challenge (iroh-relay's fallback).
    server.send(encodeChallenge(challenge));
    await this.ensureAlarm(now + LIMITS.authSeconds + 1);
    await this.settle(now);
    return new Response(null, { status: 101, webSocket: client, headers: { "sec-websocket-protocol": PROTOCOL_NAMES[version] } });
  }

  async webSocketMessage(ws: WebSocket, message: ArrayBuffer | string) {
    const now = nowSeconds();
    this.lastSeen.set(ws, now);
    const att = this.attachment(ws);
    if (!att || this.departed.has(ws)) return this.closeSocket(ws, 1011, "relay_state_lost");
    if (typeof message === "string" || message.byteLength > LIMITS.frameBytes) return this.closeSocket(ws, 1009, "invalid_frame");
    const frame = new Uint8Array(message);
    const refused = this.charge(att.ep ? [GLOBAL, att.ip, "ep:" + att.ep] : [GLOBAL, att.ip], frame.byteLength, now);
    if (refused?.daily) {
      // Closing every socket of the exhausted key prevents reconnecting to
      // bypass its budget; the persisted quota also gates reconnects.
      this.exhausted(refused.key);
      return;
    }
    try {
      if (!att.ep) {
        if (refused) return this.closeSocket(ws, 4008, "relay_quota");
        return await this.authenticate(ws, att, frame, now);
      }
      if (refused) {
        // Over a rate bucket: drop the message like a congested path would and
        // tell the client once (iroh-relay throttles reads instead).
        if (!att.limited) {
          att.limited = true;
          this.save(ws, att);
          this.send(ws, encodeStatus(att.version, STATUS.RateLimited));
        }
        return;
      }
      const parsed = parseClientFrame(frame);
      switch (parsed.kind) {
        case "datagram":
          return this.forward(ws, att, frame, hex(parsed.destination));
        case "ping":
          this.send(ws, encodePong(parsed.data));
          return;
        case "pong":
          // This relay does not ping: clients ping every 15 s, and server pings
          // would wake a hibernated object for nothing.
          return;
        default:
          // iroh-relay ends the connection on any frame it cannot handle.
          return this.closeSocket(ws, 1002, "protocol_error");
      }
    } finally {
      await this.settle(now);
    }
  }

  private async authenticate(ws: WebSocket, att: Attachment, frame: Uint8Array, now: number) {
    const parsed = parseClientFrame(frame);
    if (parsed.kind !== "auth" || !att.challenge || this.verifying.has(ws)) return this.closeSocket(ws, 1002, "handshake_expected");
    this.verifying.add(ws);
    const valid = await verifyClientAuth(parsed.publicKey, parsed.signature, unhex(att.challenge));
    this.verifying.delete(ws);
    if (ws.readyState !== OPEN || this.departed.has(ws)) return;
    const ep = hex(parsed.publicKey);
    const tags = this.ctx.getTags(ws);
    const tagged = tags.includes("ep:" + ep) || tags.includes(ANON);
    if (!valid || !tagged) {
      this.send(ws, encodeDeny(valid ? "client auth header names a different endpoint" : "signature invalid"));
      return this.closeSocket(ws, 1008, "relay_auth_denied");
    }
    const previous = this.connections(ep).filter(([other]) => other !== ws);
    const denial = this.admit(["ep:" + ep], now, () => previous.length);
    if (denial) {
      this.send(ws, encodeDeny("relay quota"));
      return this.closeSocket(ws, 4008, "relay_quota");
    }
    delete att.challenge;
    att.ep = ep;
    att.seq = this.nextSeq();
    this.save(ws, att);
    // Newest wins, as in iroh-relay: the displaced connection stays open but no
    // longer receives datagrams, and is told so.
    const displaced = previous.at(-1);
    if (displaced) this.send(displaced[0], encodeStatus(displaced[1].version, STATUS.SameEndpointIdConnected));
    this.active.set(ep, ws);
    this.send(ws, encodeConfirm());
  }

  private forward(ws: WebSocket, att: Attachment, frame: Uint8Array, destination: string) {
    const target = this.activeFor(destination);
    // Like iroh-relay: a datagram for an endpoint that is not connected is dropped.
    if (!target) return;
    if (!this.send(target, relayDatagram(frame, unhex(att.ep!)))) {
      this.closeSocket(target, 1011, "relay_send_failed");
      return;
    }
    if (!att.sentTo?.includes(destination)) {
      att.sentTo = [...(att.sentTo ?? []), destination].slice(-SENT_TO_LIMIT);
      this.save(ws, att);
    }
  }

  /** A connection ended: promote the next connection or tell peers it is gone. */
  private gone(ws: WebSocket) {
    if (this.departed.has(ws)) return;
    this.departed.add(ws);
    const att = this.attachment(ws);
    if (!att?.ep) return;
    const ep = att.ep;
    if (this.active.get(ep) === ws) this.active.delete(ep);
    const remaining = this.connections(ep);
    const next = remaining.at(-1);
    if (next) {
      const [nextWs, nextAtt] = next;
      const wasActive = att.seq! > nextAtt.seq!;
      const merged = [...new Set([...(nextAtt.sentTo ?? []), ...(att.sentTo ?? [])])].slice(-SENT_TO_LIMIT);
      if (merged.length !== (nextAtt.sentTo?.length ?? 0)) {
        nextAtt.sentTo = merged;
        this.save(nextWs, nextAtt);
      }
      if (wasActive) {
        this.active.set(ep, nextWs);
        this.send(nextWs, encodeStatus(nextAtt.version, STATUS.Healthy));
      }
      return;
    }
    const key = unhex(ep);
    for (const peer of att.sentTo ?? []) {
      const target = this.activeFor(peer);
      if (target) this.send(target, encodeEndpointGone(key));
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string) {
    try {
      ws.close(code === 1005 || code === 1006 ? 1000 : code, reason);
    } catch {
      /* already closed */
    }
    this.gone(ws);
  }

  async webSocketError(ws: WebSocket) {
    this.closeSocket(ws, 1011, "relay_error");
  }

  /** Closes only the connections charged to an exhausted key. */
  private exhausted(key: string) {
    const victims = key === GLOBAL ? this.open() : key.startsWith("ep:") ? this.connections(key.slice(3)).map(([ws]) => ws) : this.open(key);
    for (const ws of victims) this.closeSocket(ws, 4008, "relay_quota");
  }

  private sweepHandshakes(now: number) {
    for (const ws of this.open()) {
      const att = this.attachment(ws);
      if (att && !att.ep && now - att.since > LIMITS.authSeconds) this.closeSocket(ws, 4001, "relay_auth_timeout");
    }
  }

  private sweepStale(now: number) {
    for (const ws of this.open()) {
      const seen = this.lastSeen.get(ws);
      // After an eviction nothing is known: start counting now.
      if (seen === undefined) this.lastSeen.set(ws, now);
      else if (now - seen > STALE_SECONDS) this.closeSocket(ws, 4000, "relay_idle_timeout");
    }
  }

  async alarm() {
    this.alarmAt = null;
    const now = nowSeconds();
    this.flush(now, true);
    this.sweepHandshakes(now);
    this.sweepStale(now);
    // Once a day, drop persisted counters of clients not seen today.
    const today = Math.floor(now / 86400);
    if (this.sweptDay !== today) {
      this.sweptDay = today;
      for (const [key, q] of this.ctx.storage.kv.list<Quota>({ prefix: "quota:" })) {
        if (q.day < today && !this.quotas.has(key.slice("quota:".length))) this.ctx.storage.kv.delete(key);
      }
    }
    for (const [key, q] of this.quotas) {
      if (key !== GLOBAL && !this.dirty.has(key) && q.minute < Math.floor(now / 60) - 1) this.quotas.delete(key);
    }
    const sockets = this.open();
    let next = sockets.length ? now + SWEEP_SECONDS : Infinity;
    for (const ws of sockets) {
      const att = this.attachment(ws);
      if (att && !att.ep) next = Math.min(next, att.since + LIMITS.authSeconds + 1);
    }
    if (Number.isFinite(next)) await this.ensureAlarm(Math.max(next, now + 1));
  }

  /**
   * Operator restart: tells every client the relay is restarting (iroh-relay's
   * Restarting frame, advisory) and closes all connections.
   */
  restart(): number {
    const sockets = this.open();
    for (const ws of sockets) {
      this.send(ws, encodeRestarting(Math.floor(Math.random() * 5000), 15000));
      this.closeSocket(ws, 1012, "relay_restart");
    }
    this.flush(nowSeconds(), true);
    return sockets.length;
  }
}
