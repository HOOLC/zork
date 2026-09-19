import { Buffer } from "node:buffer";
import dns from "dns-packet";

const ALPHABET = "ybndrfg8ejkmcpqxot1uwisza345h769";
export const MAX_PAYLOAD = 1072;
export const MAX_AGE_MS = 24 * 60 * 60 * 1000;

export function decodeKey(value: string): Uint8Array<ArrayBuffer> {
  if (!/^[ybndrfg8ejkmcpqxot1uwisza345h769]{52}$/.test(value)) {
    throw new Error("invalid public key");
  }
  const result = new Uint8Array(32);
  let bits = 0;
  let acc = 0;
  let offset = 0;
  for (const ch of value) {
    acc = (acc << 5) | ALPHABET.indexOf(ch);
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      result[offset++] = (acc >>> bits) & 255;
    }
  }
  if ((acc & ((1 << bits) - 1)) !== 0) throw new Error("noncanonical public key");
  return result;
}

export async function readPayload(request: Request): Promise<Uint8Array<ArrayBuffer>> {
  const reader = request.body?.getReader();
  if (!reader) throw new Error("missing payload");
  const output = new Uint8Array(MAX_PAYLOAD);
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) return output.slice(0, size);
      if (size + value.length > MAX_PAYLOAD) throw new Error("payload too large");
      output.set(value, size);
      size += value.length;
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}

// Wire format and BEP44 signing preimage match iroh-dns 1.1.0 src/pkarr.rs.
export async function verifyPayload(key: Uint8Array<ArrayBuffer>, payload: Uint8Array<ArrayBuffer>, now = Date.now()): Promise<bigint> {
  if (payload.length < 84 || payload.length > MAX_PAYLOAD) throw new Error("invalid payload size");
  const timestamp = new DataView(payload.buffer, payload.byteOffset + 64, 8).getBigUint64(0);
  if (timestamp > BigInt(now + 300_000) * 1000n || timestamp < BigInt(now - MAX_AGE_MS) * 1000n) {
    throw new Error("timestamp outside accepted window");
  }
  const packet = payload.slice(72);
  const prefix = new TextEncoder().encode(`3:seqi${timestamp}e1:v${packet.length}:`);
  const signed = new Uint8Array(prefix.length + packet.length);
  signed.set(prefix);
  signed.set(packet, prefix.length);
  const publicKey = await crypto.subtle.importKey("raw", key, "Ed25519", false, ["verify"]);
  if (!(await crypto.subtle.verify("Ed25519", publicKey, payload.slice(0, 64), signed))) {
    throw new Error("invalid signature");
  }
  dns.decode(Buffer.from(packet));
  if (dns.decode.bytes !== packet.length) throw new Error("trailing DNS data");
  return timestamp;
}
