// A minimal iroh relay client (iroh-relay 1.1 wire format) for Worker tests,
// using real ed25519 keys so the Worker verifies genuine signatures.
import crypto from "node:crypto";
import { deriveKey, CHALLENGE_DOMAIN, hex } from "../src/relay-protocol.ts";
import type { harness } from "./harness.ts";

type Harness = Awaited<ReturnType<typeof harness>>;
export type Identity = { privateKey: crypto.KeyObject; id: Uint8Array };

export function identity(): Identity {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const id = new Uint8Array(Buffer.from(publicKey.export({ format: "jwk" }).x!, "base64url"));
  return { privateKey, id };
}
export const sign = (who: Identity, challenge: Uint8Array) => new Uint8Array(crypto.sign(null, deriveKey(CHALLENGE_DOMAIN, challenge), who.privateKey));
export const clientAuth = (id: Uint8Array, signature: Uint8Array) => Uint8Array.of(1, ...id, 64, ...signature);
/** The x-iroh-relay-client-auth-v1 header layout (its signature is unverifiable behind Cloudflare). */
export const authHeader = (id: Uint8Array) => Buffer.from(Uint8Array.of(...id, 64, ...new Uint8Array(64), ...new Uint8Array(16))).toString("base64url");

export type Connected = Awaited<ReturnType<typeof connectRaw>>;

/** Opens a relay WebSocket and queues every frame it receives. */
export async function connectRaw(h: Harness, options: { ip?: string; protocol?: string; headers?: Record<string, string>; path?: string } = {}): Promise<Open> {
  const response = await h.fetch(options.path ?? "/relay", {
    headers: {
      upgrade: "websocket",
      "sec-websocket-protocol": options.protocol ?? "iroh-relay-v2, iroh-relay-v1",
      ...(options.ip ? { "cf-connecting-ip": options.ip } : {}),
      ...options.headers,
    },
  });
  // Callers check `status` first; a refused upgrade has no socket fields.
  if (response.status !== 101) return { status: response.status, response } as unknown as Open;
  const socket = response.webSocket!;
  socket.accept();
  const queue: Uint8Array[] = [];
  const waiters: Array<(frame: Uint8Array) => void> = [];
  let closeCode: number | undefined;
  const closed = new Promise<number>((resolve) =>
    socket.addEventListener("close", (event: any) => {
      closeCode = event.code;
      resolve(event.code);
    }),
  );
  socket.addEventListener("message", (event: any) => {
    const frame = new Uint8Array(event.data as ArrayBuffer);
    const waiter = waiters.shift();
    if (waiter) waiter(frame);
    else queue.push(frame);
  });
  const next = (timeout = 3000) =>
    new Promise<Uint8Array>((resolve, reject) => {
      const queued = queue.shift();
      if (queued) return resolve(queued);
      const timer = setTimeout(() => {
        const index = waiters.indexOf(done);
        if (index >= 0) waiters.splice(index, 1);
        reject(new Error("no frame within " + timeout + "ms"));
      }, timeout);
      const done = (frame: Uint8Array) => {
        clearTimeout(timer);
        resolve(frame);
      };
      waiters.push(done);
    });
  const open = {
    status: response.status as number,
    response,
    socket,
    protocol: response.headers.get("sec-websocket-protocol"),
    queue,
    next,
    closed,
    get closeCode() {
      return closeCode;
    },
    send: (frame: Uint8Array | string) => socket.send(frame as any),
    close: () => socket.close(),
  };
  return open;
}
type Open = {
  status: number;
  response: Awaited<ReturnType<Harness["fetch"]>>;
  socket: any;
  protocol: string | null;
  queue: Uint8Array[];
  next: (timeout?: number) => Promise<Uint8Array>;
  closed: Promise<number>;
  readonly closeCode: number | undefined;
  send: (frame: Uint8Array | string) => void;
  close: () => void;
};

/** Connects and completes the signed-challenge handshake. */
export async function connect(h: Harness, who: Identity = identity(), options: Parameters<typeof connectRaw>[1] & { header?: boolean } = {}) {
  const c = await connectRaw(h, { ...options, headers: { ...(options.header ? { "x-iroh-relay-client-auth-v1": authHeader(who.id) } : {}), ...options.headers } });
  if (c.status !== 101) throw new Error("relay upgrade: " + c.status);
  const challenge = await c.next();
  if (challenge[0] !== 0 || challenge.length !== 17) throw new Error("expected ServerChallenge");
  c.send(clientAuth(who.id, sign(who, challenge.subarray(1))));
  const confirm = await c.next();
  if (confirm.length !== 1 || confirm[0] !== 2) throw new Error("expected ServerConfirmsAuth, got " + hex(confirm));
  return Object.assign(c, { who, id: who.id });
}

export const datagram = (to: Uint8Array, payload: Uint8Array, ecn = 0) => Uint8Array.of(4, ...to, ecn, ...payload);
export const batch = (to: Uint8Array, segment: number, payload: Uint8Array, ecn = 0) => Uint8Array.of(5, ...to, ecn, segment >> 8, segment & 0xff, ...payload);
export const ping = (data: Uint8Array) => Uint8Array.of(9, ...data);

/** Proves nothing else is queued: a ping's pong is the next frame. */
export async function nothingPending(c: { send(frame: Uint8Array): void; next(timeout?: number): Promise<Uint8Array> }) {
  const data = crypto.getRandomValues(new Uint8Array(8));
  c.send(ping(data));
  const frame = await c.next();
  return frame[0] === 10 && Buffer.from(frame.subarray(1)).equals(Buffer.from(data));
}
