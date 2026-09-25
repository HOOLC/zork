import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { limited, nowSeconds, reply } from "./auth";

// Relay admission is a cost bound, never a Mesh membership authority.
//
// Budgets are kept per client key so one noisy client (or a household of
// devices behind one NAT) cannot lock everybody else out:
//   - "ip:<addr>"   every upgrade, keyed by CF-Connecting-IP (IPv6 by /64 so a
//                   host cannot rotate through its own prefix);
//   - "ep:<hex>"    the iroh EndpointId, known only once the relay process has
//                   verified the client's signed challenge (see RelayBudget);
//   - "global"      a service-wide safety cap sized to operating cost.
// The unauthenticated `x-iroh-relay-client-auth-v1` upgrade header also names
// an EndpointId, but it cannot be verified here (it signs TLS keying material of
// the client<->Cloudflare session) and EndpointIds are public (signed
// discovery), so keying admission on it would let anyone lock a victim out.
//
// A normal Zork process holds 1-3 relay connections (device endpoint, an
// enrollment endpoint while invites are open, short-lived client endpoints), so
// a single endpoint needs 1 and briefly 2 while reconnecting.
//
// Global cap, justified against Cloudflare pricing (Workers Paid, 2026):
//   - frames: every WebSocket message through this Durable Object is billed as a
//     request at 20:1. 40M frames/day = 2M billed requests/day ≈ 60M/month ≈ $9
//     at $0.15/M. That is the dominant variable cost, so it is the tightest cap.
//   - bytes: 20 GiB/day ≈ 600 GiB/month stays inside the container's included
//     1 TB egress (beyond it $0.025/GB, i.e. at most ~$15/month more).
//   - connections: all sockets are proxied by this single object (128 MB,
//     one thread) and one "lite" relay container (1/16 vCPU, 256 MiB). 256
//     concurrent sockets ≈ 80-120 online Zork processes keeps both far from
//     memory limits; duration cost of one always-on object and container is
//     ~$5/month regardless of the socket count.
//   - storage: SQLite rows written are billed ($1/M beyond 50M/month), so byte
//     and frame counters are held in memory and flushed at most every
//     FLUSH_SECONDS; connects and quota exhaustion are written immediately.
//     A restart can forget at most a few seconds of traffic accounting.
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
    connections: 256,
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
  // One device identity: its connection plus reconnect overlap (the relay keeps
  // a displaced connection open as inactive until it closes).
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
  frameBytes: 128 * 1024,
  // A connection must finish the relay's signed challenge in this time, so an
  // unauthenticated socket cannot sit on IP capacity without an identity.
  authSeconds: 30,
} satisfies Record<"global" | "ip" | "endpoint", Budget> & Record<string, unknown>;
const FLUSH_SECONDS = 10;
// Retry-After for concurrency rejections: long enough to stop a reconnect
// storm, short enough that a reconnecting device recovers quickly.
const BUSY_RETRY_SECONDS = 30;
const GLOBAL = "global";

type Quota = {
  minute: number;
  connects: number;
  day: number;
  bytes: number;
  at: number;
  balance: number;
  frameBalance?: number;
  frames?: number;
};
type Connection = { keys: string[]; close: (code: number, reason: string) => void };
type Denial = { key: string; retryAfter: number };

// iroh-relay handshake frames (QUIC varint tag, single byte for these tags).
const CLIENT_AUTH = 1,
  SERVER_CONFIRMS_AUTH = 2;

export function budgetFor(key: string): Budget {
  return key === GLOBAL ? LIMITS.global : key.startsWith("ep:") ? LIMITS.endpoint : LIMITS.ip;
}
function storageKey(key: string) {
  // The pre-per-client deployment stored its single budget as "quota"; keep it
  // as the global key so an upgrade does not reset the day's consumption.
  return key === GLOBAL ? "quota" : "quota:" + key;
}

/** Client IP budget key; IPv6 addresses share their /64. */
export function ipKey(request: Request): string {
  const raw = request.headers.get("cf-connecting-ip")?.trim() ?? "";
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(raw)) return "ip:" + raw;
  if (raw.includes(":") && /^[0-9a-fA-F:.]+$/.test(raw)) {
    const [head, tail = ""] = raw.toLowerCase().split("::");
    const left = head ? head.split(":") : [];
    const right = raw.includes("::") && tail ? tail.split(":") : [];
    const groups = raw.includes("::") ? [...left, ...Array(Math.max(0, 8 - left.length - right.length)).fill("0"), ...right] : left;
    return "ip:" + groups.slice(0, 4).map((g) => g.replace(/^0+(?=.)/, "")).join(":") + "::/64";
  }
  // Without Cloudflare's client address (local tests), all clients share one key.
  return "ip:unknown";
}

function secondsUntilTomorrow(now: number) {
  return 86400 - (now % 86400);
}

export class RelayBudget extends DurableObject<Env> {
  private connections = new Set<Connection>();
  private live = new Map<string, Set<Connection>>();
  private quotas = new Map<string, Quota>();
  private dirty = new Set<string>();
  private flushAt = 0;

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
        at: now,
        balance: limits.burstBytes,
        frameBalance: limits.burstFrames,
        frames: 0,
      };
      this.quotas.set(key, q);
    }
    if (q.minute !== minute) {
      q.minute = minute;
      q.connects = 0;
    }
    if (q.day !== day) {
      q.day = day;
      q.bytes = 0;
      q.frames = 0;
    }
    q.frames ??= 0;
    const elapsed = Math.max(0, now - q.at);
    q.frameBalance = Math.min(limits.burstFrames, (q.frameBalance ?? limits.burstFrames) + elapsed * limits.framesPerSecond);
    q.balance = Math.min(limits.burstBytes, q.balance + elapsed * limits.bytesPerSecond);
    q.at = now;
    return q;
  }

  protected store(key: string, q: Quota) {
    this.quotas.set(key, q);
    this.dirty.delete(key);
    this.ctx.storage.kv.put(storageKey(key), q);
  }

  /** Charges a connect to every key, all or nothing. */
  private admit(keys: string[]): Denial | null {
    const now = nowSeconds();
    const quotas = keys.map((key) => [key, this.quota(key, now)] as const);
    for (const [key, q] of quotas) {
      const limits = budgetFor(key);
      if ((this.live.get(key)?.size ?? 0) >= limits.connections) return { key, retryAfter: BUSY_RETRY_SECONDS };
      if (q.connects >= limits.connectsPerMinute) return { key, retryAfter: 60 - (now % 60) };
      if (q.bytes >= limits.bytesPerDay || (q.frames ?? 0) >= limits.framesPerDay) return { key, retryAfter: secondsUntilTomorrow(now) };
    }
    for (const [key, q] of quotas) {
      q.connects++;
      this.store(key, q);
    }
    return null;
  }

  /**
   * Charges one frame to every key, all or nothing. Returns the key whose
   * budget refused it: `daily` when its day is used up, otherwise its
   * per-second rate bucket is momentarily empty.
   */
  private charge(keys: string[], amount: number): { key: string; daily: boolean } | null {
    const now = nowSeconds();
    const quotas = keys.map((key) => [key, this.quota(key, now)] as const);
    for (const [key, q] of quotas) {
      const limits = budgetFor(key);
      // Tiny/empty frames consume CPU and billable events too.
      if (q.bytes + amount > limits.bytesPerDay || (q.frames ?? 0) >= limits.framesPerDay) {
        // Mark the day used up so reconnects are refused at admission too.
        q.bytes = Math.max(q.bytes, limits.bytesPerDay);
        this.store(key, q);
        return { key, daily: true };
      }
      if (amount > q.balance || (q.frameBalance ?? 0) < 1) return { key, daily: false };
    }
    for (const [key, q] of quotas) {
      q.frameBalance = (q.frameBalance ?? 0) - 1;
      q.frames = (q.frames ?? 0) + 1;
      q.bytes += amount;
      q.balance -= amount;
      this.dirty.add(key);
    }
    if (now >= this.flushAt) this.flush(now);
    else void this.scheduleFlush();
    return null;
  }

  private flush(now = nowSeconds()) {
    for (const key of this.dirty) {
      const q = this.quotas.get(key);
      if (q) this.ctx.storage.kv.put(storageKey(key), q);
    }
    this.dirty.clear();
    this.flushAt = now + FLUSH_SECONDS;
    // Forget idle clients from memory; their persisted quota stays until the
    // daily sweep so a reconnect in the same day still sees its consumption.
    const minute = Math.floor(now / 60);
    for (const [key, q] of this.quotas) {
      if (key !== GLOBAL && !this.live.get(key)?.size && q.minute < minute) this.quotas.delete(key);
    }
  }

  private scheduled = false;
  private async scheduleFlush() {
    if (this.scheduled) return;
    this.scheduled = true;
    if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(Date.now() + FLUSH_SECONDS * 1000);
  }

  async alarm() {
    this.scheduled = false;
    const now = nowSeconds();
    this.flush(now);
    // Daily sweep of persisted quotas for clients not seen today.
    const today = Math.floor(now / 86400);
    for (const [key, q] of this.ctx.storage.kv.list<Quota>({ prefix: "quota:" })) {
      if (q.day < today && !this.quotas.has(key.slice("quota:".length))) this.ctx.storage.kv.delete(key);
    }
    if (this.dirty.size) await this.scheduleFlush();
  }

  private track(connection: Connection, key: string) {
    connection.keys.push(key);
    let set = this.live.get(key);
    if (!set) this.live.set(key, (set = new Set()));
    set.add(connection);
  }
  private untrack(connection: Connection) {
    this.connections.delete(connection);
    for (const key of connection.keys) {
      const set = this.live.get(key);
      set?.delete(connection);
      if (set && !set.size) this.live.delete(key);
    }
  }
  /** Closes only the connections charged to an exhausted key. */
  private exhausted(key: string) {
    const victims = key === GLOBAL ? [...this.connections] : [...(this.live.get(key) ?? [])];
    for (const c of victims) c.close(4008, "relay_quota");
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return reply({ error: "websocket_required" }, 426);
    const ip = ipKey(request);
    const denial = this.admit([GLOBAL, ip]);
    if (denial) return limited(denial.retryAfter);
    let server: WebSocket | undefined, upstream: WebSocket | undefined;
    let dialTimer: ReturnType<typeof setTimeout> | undefined;
    let authTimer: ReturnType<typeof setTimeout> | undefined;
    let dialAbort: AbortController | undefined;
    let closed = false;
    const connection: Connection = {
      keys: [],
      close: (code, reason) => {
        if (closed) return;
        closed = true;
        if (dialTimer !== undefined) clearTimeout(dialTimer);
        if (authTimer !== undefined) clearTimeout(authTimer);
        dialAbort?.abort();
        for (const socket of [server, upstream]) {
          try {
            socket?.close(code, reason);
          } catch {
            /* already closed */
          }
        }
        this.untrack(connection);
      },
    };
    // Reserve before awaiting the container, including in-flight upgrades in
    // the budgets. Cloud account state does not govern transport.
    this.connections.add(connection);
    this.track(connection, GLOBAL);
    this.track(connection, ip);
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
      if (closed) {
        upstream.close(1011, "relay_unavailable");
        connection.close(1011, "relay_unavailable");
        return reply({ error: "relay_unavailable" }, 502);
      }
      const pair = new WebSocketPair();
      server = pair[1];
      server.binaryType = "arraybuffer";
      server.accept();
      // The relay process verifies the client's signature over its challenge;
      // its ServerConfirmsAuth after a ClientAuth frame names a proven
      // EndpointId. Only then is the endpoint budget applied.
      let claimed: string | undefined;
      let authenticated = false;
      authTimer = setTimeout(() => connection.close(4001, "relay_auth_timeout"), LIMITS.authSeconds * 1000);
      const observe = (fromClient: boolean, frame: Uint8Array) => {
        if (authenticated || frame.length === 0) return true;
        if (fromClient && frame[0] === CLIENT_AUTH && frame.length >= 33) {
          claimed ??= "ep:" + Array.from(frame.subarray(1, 33), (b) => b.toString(16).padStart(2, "0")).join("");
        } else if (!fromClient && frame[0] === SERVER_CONFIRMS_AUTH && claimed) {
          authenticated = true;
          clearTimeout(authTimer);
          authTimer = undefined;
          const denial = this.admit([claimed]);
          if (denial) {
            connection.close(4008, "relay_quota");
            return false;
          }
          this.track(connection, claimed);
        }
        return true;
      };
      const forward = (from: WebSocket, to: WebSocket, fromClient: boolean) => {
        from.addEventListener("message", (event) => {
          if (closed) return;
          if (!(event.data instanceof ArrayBuffer) || event.data.byteLength > LIMITS.frameBytes) {
            connection.close(1009, "invalid_frame");
            return;
          }
          const refused = this.charge(connection.keys, event.data.byteLength);
          if (refused?.daily) {
            // Closing every socket of the exhausted key prevents reconnecting
            // to bypass its budget; the persisted quota also gates reconnects.
            this.exhausted(refused.key);
            return;
          }
          if (refused) {
            // A burst over a rate bucket closes only the sending connection.
            connection.close(4008, "relay_quota");
            return;
          }
          // Confirmation is forwarded only after the endpoint was admitted.
          if (!observe(fromClient, new Uint8Array(event.data))) return;
          try {
            to.send(event.data);
          } catch {
            connection.close(1011, "relay_send_failed");
          }
        });
        from.addEventListener("close", () => connection.close(1000, "relay_closed"));
        from.addEventListener("error", () => connection.close(1011, "relay_error"));
      };
      forward(server, upstream, true);
      forward(upstream, server, false);
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
