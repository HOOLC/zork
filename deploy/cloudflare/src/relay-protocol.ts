// Wire format of the iroh relay protocol as spoken by iroh-relay 1.1 (the crate
// in Cargo.lock). Sources: iroh-relay src/protos/{common,handshake,relay}.rs and
// src/http.rs. Every relay message is one binary WebSocket message:
//
//   QUIC varint frame type || payload
//
// Handshake (postcard-encoded payloads):
//   0 ServerChallenge    16 random bytes
//   1 ClientAuth         32-byte public key || LEB128 len (=64) || 64-byte ed25519
//                        signature over blake3::derive_key(DOMAIN, challenge)
//   2 ServerConfirmsAuth (empty)
//   3 ServerDeniesAuth   LEB128 len || UTF-8 reason
// Relaying (fixed layouts):
//   4/5 ClientToRelayDatagram[Batch]  dst key(32) || ECN(1) || [segment size u16 BE] || data
//   6/7 RelayToClientDatagram[Batch]  src key(32) || same tail as 4/5
//   8 EndpointGone  key(32)          9 Ping / 10 Pong  8 bytes
//   11 Health (v1 only) UTF-8        12 Restarting  u32 BE ms reconnect_in, u32 BE ms try_for
//   13 Status (v2 only) 1 byte: 0 healthy, 1 same endpoint id connected, 2 rate limited
//
// The upgrade negotiates `Sec-WebSocket-Protocol` iroh-relay-v2 / iroh-relay-v1.

export const FRAME = {
  ServerChallenge: 0,
  ClientAuth: 1,
  ServerConfirmsAuth: 2,
  ServerDeniesAuth: 3,
  ClientToRelayDatagram: 4,
  ClientToRelayDatagramBatch: 5,
  RelayToClientDatagram: 6,
  RelayToClientDatagramBatch: 7,
  EndpointGone: 8,
  Ping: 9,
  Pong: 10,
  Health: 11,
  Restarting: 12,
  Status: 13,
} as const;

/** Largest payload after the frame type that iroh-relay accepts (MAX_PACKET_SIZE). */
export const MAX_PACKET_SIZE = 64 * 1024;
/** iroh-relay's WebSocket message limit (MAX_FRAME_SIZE). */
export const MAX_FRAME_SIZE = 1024 * 1024;
export const KEY_LENGTH = 32;
export const CHALLENGE_DOMAIN = "iroh-relay handshake v1 challenge signature";
export const CLIENT_AUTH_HEADER = "x-iroh-relay-client-auth-v1";

export type ProtocolVersion = 1 | 2;
export const PROTOCOL_NAMES: Record<ProtocolVersion, string> = { 1: "iroh-relay-v1", 2: "iroh-relay-v2" };

export const STATUS = { Healthy: 0, SameEndpointIdConnected: 1, RateLimited: 2 } as const;
type StatusCode = (typeof STATUS)[keyof typeof STATUS];
// Display strings of iroh-relay's Status, sent as Health text to v1 clients.
const STATUS_TEXT: Record<StatusCode, string> = {
  0: "The connection is healthy and has recovered from previous problems",
  1: "Another endpoint connected with the same endpoint id. No more messages will be received.",
  2: "The relay is rate-limiting this endpoint; outbound relay traffic is being throttled.",
};

/** Picks the newest supported version from a comma-separated subprotocol offer. */
export function negotiate(offer: string | null): ProtocolVersion | null {
  let best: ProtocolVersion | null = null;
  for (const item of (offer ?? "").split(",")) {
    const name = item.trim();
    const version = name === "iroh-relay-v2" ? 2 : name === "iroh-relay-v1" ? 1 : null;
    if (version && (!best || version > best)) best = version;
  }
  return best;
}

/** QUIC variable-length integer (RFC 9000 §16), as used for frame types. */
export function readVarint(bytes: Uint8Array): { value: number; length: number } | null {
  if (!bytes.length) return null;
  const length = 1 << (bytes[0] >> 6);
  if (bytes.length < length) return null;
  let value = bytes[0] & 0x3f;
  for (let i = 1; i < length; i++) value = value * 256 + bytes[i];
  return { value, length };
}

function leb128(value: number): number[] {
  const out: number[] = [];
  do {
    let byte = value & 0x7f;
    value = Math.floor(value / 128);
    if (value) byte |= 0x80;
    out.push(byte);
  } while (value);
  return out;
}

const encoder = new TextEncoder();

export const encodeChallenge = (challenge: Uint8Array) => Uint8Array.of(FRAME.ServerChallenge, ...challenge);
export const encodeConfirm = () => Uint8Array.of(FRAME.ServerConfirmsAuth);
export function encodeDeny(reason: string): Uint8Array {
  const text = encoder.encode(reason);
  return Uint8Array.of(FRAME.ServerDeniesAuth, ...leb128(text.length), ...text);
}
export const encodePing = (data: Uint8Array) => Uint8Array.of(FRAME.Ping, ...data);
export const encodePong = (data: Uint8Array) => Uint8Array.of(FRAME.Pong, ...data);
export const encodeEndpointGone = (key: Uint8Array) => Uint8Array.of(FRAME.EndpointGone, ...key);
export function encodeRestarting(reconnectInMs: number, tryForMs: number): Uint8Array {
  const out = new Uint8Array(9);
  const view = new DataView(out.buffer);
  out[0] = FRAME.Restarting;
  view.setUint32(1, reconnectInMs);
  view.setUint32(5, tryForMs);
  return out;
}
/** Connection status in the form the negotiated version understands. */
export function encodeStatus(version: ProtocolVersion, status: StatusCode): Uint8Array | null {
  if (version === 2) return Uint8Array.of(FRAME.Status, status);
  // iroh-relay does not tell v1 clients about rate limiting.
  return status === STATUS.RateLimited ? null : Uint8Array.of(FRAME.Health, ...encoder.encode(STATUS_TEXT[status]));
}

/** Turns a client's datagram frame into the frame its destination receives. */
export function relayDatagram(frame: Uint8Array, source: Uint8Array): Uint8Array {
  const out = frame.slice();
  out[0] = frame[0] + (FRAME.RelayToClientDatagram - FRAME.ClientToRelayDatagram);
  out.set(source, 1);
  return out;
}

export type ClientFrame = { kind: "datagram"; destination: Uint8Array; batch: boolean } | { kind: "ping"; data: Uint8Array } | { kind: "pong"; data: Uint8Array } | { kind: "auth"; publicKey: Uint8Array; signature: Uint8Array } | { kind: "invalid"; reason: string };

/** Parses a message a client sends; mirrors ClientToRelayMsg::from_bytes plus ClientAuth. */
export function parseClientFrame(frame: Uint8Array): ClientFrame {
  const tag = readVarint(frame);
  if (!tag) return { kind: "invalid", reason: "empty" };
  const body = frame.subarray(tag.length);
  if (body.length > MAX_PACKET_SIZE) return { kind: "invalid", reason: "frame_too_large" };
  switch (tag.value) {
    case FRAME.ClientToRelayDatagram:
    case FRAME.ClientToRelayDatagramBatch: {
      const batch = tag.value === FRAME.ClientToRelayDatagramBatch;
      // Key, ECN byte, and for batches a u16 segment size.
      if (body.length < KEY_LENGTH + (batch ? 3 : 1)) return { kind: "invalid", reason: "short_datagram" };
      // Relaying rewrites the first byte in place, so only 1-byte tags are valid here (always true for 4/5).
      if (tag.length !== 1) return { kind: "invalid", reason: "non_minimal_tag" };
      return { kind: "datagram", destination: body.subarray(0, KEY_LENGTH), batch };
    }
    case FRAME.Ping:
    case FRAME.Pong:
      if (body.length !== 8) return { kind: "invalid", reason: "bad_ping" };
      return { kind: tag.value === FRAME.Ping ? "ping" : "pong", data: body.slice() };
    case FRAME.ClientAuth: {
      // postcard: [u8; 32] as a tuple (raw), then serde_bytes [u8; 64] (LEB128 length + bytes).
      if (body.length < KEY_LENGTH + 1 + 64 || body[KEY_LENGTH] !== 64) return { kind: "invalid", reason: "bad_client_auth" };
      return { kind: "auth", publicKey: body.slice(0, KEY_LENGTH), signature: body.slice(KEY_LENGTH + 1, KEY_LENGTH + 65) };
    }
    default:
      return { kind: "invalid", reason: "unexpected_frame_type" };
  }
}

/**
 * The EndpointId a client names in its `x-iroh-relay-client-auth-v1` header
 * (postcard KeyMaterialClientAuth: key(32) || len(64) || signature(64) || suffix(16),
 * base64url without padding). The signature covers TLS keying material of the
 * client's session with Cloudflare's edge, which this server cannot export, so the
 * claim is unverified: it only selects a routing tag and must match the key the
 * client later proves with the challenge.
 */
export function headerClaim(value: string | null): Uint8Array | null {
  if (!value || !/^[A-Za-z0-9_-]{150,152}$/.test(value)) return null;
  try {
    const bytes = Uint8Array.from(atob(value.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
    return bytes.length === KEY_LENGTH + 1 + 64 + 16 && bytes[KEY_LENGTH] === 64 ? bytes.slice(0, KEY_LENGTH) : null;
  } catch {
    return null;
  }
}

export const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
export function unhex(text: string): Uint8Array {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** What the client signs: BLAKE3 derive_key(CHALLENGE_DOMAIN, challenge). */
export const challengeMessage = (challenge: Uint8Array) => deriveKey(CHALLENGE_DOMAIN, challenge);

/** Verifies a ClientAuth signature (ed25519) over the challenge message. */
export async function verifyClientAuth(publicKey: Uint8Array, signature: Uint8Array, challenge: Uint8Array): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey("raw", publicKey, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, signature, challengeMessage(challenge));
  } catch {
    // Not a valid curve point, or otherwise unusable key material.
    return false;
  }
}

// ---- BLAKE3 (single-chunk inputs, ≤ 1024 bytes) ----------------------------
// The handshake only hashes a 43-byte context and a 16-byte challenge; one chunk
// covers both, so the tree/parent logic of BLAKE3 is not needed.
const IV = Uint32Array.of(0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19);
const PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1,
  CHUNK_END = 2,
  ROOT = 8,
  DERIVE_KEY_CONTEXT = 32,
  DERIVE_KEY_MATERIAL = 64;

function compress(cv: Uint32Array, block: Uint32Array, blockLength: number, flags: number): Uint32Array {
  const s = Uint32Array.of(...cv, IV[0], IV[1], IV[2], IV[3], 0, 0, blockLength, flags);
  let m = block;
  const g = (a: number, b: number, c: number, d: number, x: number, y: number) => {
    s[a] = s[a] + s[b] + x;
    s[d] = ((s[d] ^ s[a]) >>> 16) | ((s[d] ^ s[a]) << 16);
    s[c] = s[c] + s[d];
    s[b] = ((s[b] ^ s[c]) >>> 12) | ((s[b] ^ s[c]) << 20);
    s[a] = s[a] + s[b] + y;
    s[d] = ((s[d] ^ s[a]) >>> 8) | ((s[d] ^ s[a]) << 24);
    s[c] = s[c] + s[d];
    s[b] = ((s[b] ^ s[c]) >>> 7) | ((s[b] ^ s[c]) << 25);
  };
  for (let round = 0; round < 7; round++) {
    g(0, 4, 8, 12, m[0], m[1]);
    g(1, 5, 9, 13, m[2], m[3]);
    g(2, 6, 10, 14, m[4], m[5]);
    g(3, 7, 11, 15, m[6], m[7]);
    g(0, 5, 10, 15, m[8], m[9]);
    g(1, 6, 11, 12, m[10], m[11]);
    g(2, 7, 8, 13, m[12], m[13]);
    g(3, 4, 9, 14, m[14], m[15]);
    m = Uint32Array.from(PERMUTATION, (i) => m[i]);
  }
  const out = new Uint32Array(8);
  for (let i = 0; i < 8; i++) out[i] = s[i] ^ s[i + 8];
  return out;
}

function hashChunk(key: Uint32Array, input: Uint8Array, mode: number): Uint32Array {
  if (input.length > 1024) throw new Error("blake3: single-chunk input only");
  let cv = key;
  const blocks = Math.max(1, Math.ceil(input.length / 64));
  for (let i = 0; i < blocks; i++) {
    const bytes = new Uint8Array(64);
    const part = input.subarray(i * 64, i * 64 + 64);
    bytes.set(part);
    const view = new DataView(bytes.buffer);
    const block = Uint32Array.from({ length: 16 }, (_, j) => view.getUint32(j * 4, true));
    const flags = mode | (i === 0 ? CHUNK_START : 0) | (i === blocks - 1 ? CHUNK_END | ROOT : 0);
    cv = compress(cv, block, part.length, flags);
  }
  return cv;
}

function words(bytes: Uint32Array): Uint8Array {
  const out = new Uint8Array(32);
  const view = new DataView(out.buffer);
  bytes.forEach((word, i) => view.setUint32(i * 4, word, true));
  return out;
}

export const blake3 = (input: Uint8Array) => words(hashChunk(IV, input, 0));
export function deriveKey(context: string, material: Uint8Array): Uint8Array {
  const contextKey = hashChunk(IV, encoder.encode(context), DERIVE_KEY_CONTEXT);
  return words(hashChunk(contextKey, material, DERIVE_KEY_MATERIAL));
}
