import { installPage } from "./install";
import { Container } from "@cloudflare/containers";
import { DurableObject } from "cloudflare:workers";
import { decodeKey, MAX_AGE_MS, readPayload, verifyPayload } from "./pkarr";
import { authConfigured, bearerToken, denied, digest, readJson, reply, validId, validSecret, verifyToken } from "./auth";
import { devicePage, googleStart, consumeLoginRate } from "./login";
import type { Env } from "./env";
export { RelayBudget } from "./relay";
export { Account } from "./account";
export { LoginAttempt, LoginLimiter } from "./login";

export class Relay extends Container<Env> {
  defaultPort = 8080;
  sleepAfter = "10m";
}

type RecordValue = { payload: Uint8Array; timestamp: string; expires: number };

// One strongly consistent object per public key; no global discovery bottleneck.
export class DiscoveryRecord extends DurableObject<Env> {
  publish(payload: Uint8Array, timestamp: string): number {
    return this.ctx.storage.transactionSync(() => {
      const old = this.ctx.storage.kv.get<RecordValue>("record");
      if (old && BigInt(timestamp) < BigInt(old.timestamp)) return 409;
      if (old && timestamp === old.timestamp) {
        // An identical retry is idempotent and does not renew an expired record.
        return old.payload.length === payload.length && old.payload.every((b, i) => b === payload[i]) ? 204 : 409;
      }
      this.ctx.storage.kv.put("record", {
        payload,
        timestamp,
        expires: Number(BigInt(timestamp) / 1000n) + MAX_AGE_MS,
      });
      return 204;
    });
  }

  resolve(): Uint8Array | null {
    const record = this.ctx.storage.kv.get<RecordValue>("record");
    // Retain the timestamp after expiry so an old signed record cannot be replayed.
    return record && record.expires > Date.now() ? record.payload : null;
  }
}

const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, PUT, OPTIONS",
  "access-control-allow-headers": "Content-Type, Authorization",
  "cache-control": "no-store",
};

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;
    if ((path === "/healthz" || path === "/ping") && request.method === "GET") {
      return Response.json({
        service: "zork-network",
        relay: "iroh-relay-1.1.0",
        google_login: authConfigured(env),
      });
    }
    if (path === "/install" && request.method === "GET") {
      if (url.origin !== env.PUBLIC_ORIGIN) return reply({ error: "invalid_origin" }, 421);
      return installPage();
    }
    if (path === "/relay") {
      if (url.origin !== env.PUBLIC_ORIGIN) return reply({ error: "invalid_origin" }, 421);
      if (request.method !== "GET") return reply({ error: "method_not_allowed" }, 405);
      return env.RELAY_BUDGET.getByName("primary").fetch(request);
    }
    if (path.startsWith("/v1/")) {
      if (url.origin !== env.PUBLIC_ORIGIN) return reply({ error: "invalid_origin" }, 421);
      if (path.startsWith("/v1/auth/") && !authConfigured(env)) return reply({ error: "login_not_configured" }, 503);
    }
    if (path === "/v1/auth/google/start" && request.method === "GET") {
      return googleStart(env, request);
    }
    if (path === "/v1/auth/device/complete" && request.method === "GET") {
      return devicePage("请返回 Zork", "<p>登录结果会在发起请求的 Zork 中显示，现在可以关闭此页。</p>");
    }
    const device = /^\/v1\/auth\/device\/([A-Za-z0-9_-]{43})$/.exec(path);
    if (device && (request.method === "GET" || request.method === "POST")) {
      return env.LOGINS.getByName(device[1]).authorizeDevice(device[1], request);
    }
    if ((path === "/v1/auth/device" || path === "/v1/auth/device/token" || path === "/v1/auth/device/cancel") && request.method === "POST") {
      try {
        const body = await readJson(request);
        if (!validSecret(body.id)) return reply({ error: "invalid_login" }, 400);
        if (path.endsWith("/token") || path.endsWith("/cancel")) {
          if (!validSecret(body.code_verifier)) return reply({ error: "invalid_grant" }, 401);
          return path.endsWith("/cancel") ? env.LOGINS.getByName(body.id).cancelDevice(body.code_verifier) : env.LOGINS.getByName(body.id).pollDevice(body.code_verifier);
        }
        if (!validSecret(body.code_challenge) || typeof body.name !== "string") return reply({ error: "invalid_login" }, 400);
        if (!(await consumeLoginRate(env, request))) return reply({ error: "rate_limited" }, 429, { "retry-after": "60" });
        return env.LOGINS.getByName(body.id).startDevice(body.id, body.code_challenge, body.name);
      } catch {
        return reply({ error: "invalid_request" }, 400);
      }
    }
    if (path === "/v1/auth/google/callback" && request.method === "GET") {
      const state = url.searchParams.get("state");
      if (!validSecret(state)) return reply({ error: "invalid_login_state" }, 400);
      return env.LOGINS.getByName(state).callback(state, request);
    }
    if (path === "/v1/auth/token" && request.method === "POST") {
      try {
        const body = await readJson(request);
        if (typeof body.code !== "string") return denied();
        const [id, secret, extra] = body.code.split(".");
        if (!validSecret(id) || !validSecret(secret) || extra !== undefined || !validSecret(body.code_verifier) || typeof body.redirect_uri !== "string") return denied();
        return env.LOGINS.getByName(id).exchange(secret, body.code_verifier, body.redirect_uri);
      } catch {
        return reply({ error: "invalid_request" }, 400);
      }
    }
    if ((path === "/v1/auth/refresh" || path === "/v1/auth/logout") && request.method === "POST") {
      try {
        const token = bearerToken(request);
        const claims = token ? await verifyToken(env, token, "refresh") : null;
        if (!claims || !token) return denied();
        const body = await readJson(request);
        const account = env.ACCOUNTS.getByName(claims.sub);
        if (path.endsWith("/refresh")) {
          if (!validId(body.request_id)) return reply({ error: "invalid_request" }, 400);
          return account.refresh(token, body.request_id);
        }
        if (typeof body.all !== "boolean") return reply({ error: "invalid_request" }, 400);
        return account.logout(token, body.all);
      } catch {
        return reply({ error: "invalid_request" }, 400);
      }
    }
    const admin = /^\/v1\/admin\/accounts\/([A-Za-z0-9_-]{1,128})$/.exec(path);
    const restartRelay = path === "/v1/admin/relay/restart";
    if ((admin || restartRelay) && request.method === "POST") {
      const supplied = bearerToken(request);
      if (!env.ADMIN_TOKEN || env.ADMIN_TOKEN.length < 43 || !supplied || (await digest(supplied)) !== (await digest(env.ADMIN_TOKEN))) return denied();
      try {
        if (restartRelay) {
          // Cutovers from stateless admission must also retire the old process
          // and its untracked sockets; a started rollout is not that guarantee.
          await env.RELAY.getByName("primary").destroy();
          return reply({ restarted: true });
        }
        const body = await readJson(request);
        if (typeof body.blocked !== "boolean") return reply({ error: "invalid_request" }, 400);
        return env.ACCOUNTS.getByName(admin![1]).administer(body.blocked);
      } catch {
        return reply({ error: "invalid_request" }, 400);
      }
    }
    if (path === "/v1/auth/devices" || path === "/v1/auth/session" || path === "/v1/auth/sessions" || /^\/v1\/auth\/sessions\/[^/]+$/.test(path)) {
      const token = bearerToken(request);
      const claims = token ? await verifyToken(env, token, "access") : null;
      if (!claims) return denied();
      return env.ACCOUNTS.getByName(claims.sub).fetch(request);
    }
    const match = /^\/pkarr\/([a-z0-9]{52})$/.exec(path);
    if (!match) return new Response("Not found", { status: 404 });
    if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: cors });
    if (request.method !== "GET" && request.method !== "PUT") {
      return new Response("Method not allowed", {
        status: 405,
        headers: { ...cors, allow: "GET, PUT, OPTIONS" },
      });
    }
    let key: Uint8Array<ArrayBuffer>;
    try {
      key = decodeKey(match[1]);
    } catch {
      return new Response("Invalid public key", { status: 400, headers: cors });
    }
    const record = env.RECORDS.getByName(match[1]);
    if (request.method === "GET") {
      const payload = await record.resolve();
      return new Response(payload ? new Uint8Array(payload) : null, {
        status: payload ? 200 : 404,
        headers: { ...cors, "content-type": "application/octet-stream" },
      });
    }
    let payload: Uint8Array<ArrayBuffer>;
    let timestamp: bigint;
    try {
      payload = await readPayload(request);
      timestamp = await verifyPayload(key, payload);
    } catch {
      return new Response("Invalid signed packet", { status: 400, headers: cors });
    }
    const status = await record.publish(payload, timestamp.toString());
    return new Response(null, { status, headers: cors });
  },
} satisfies ExportedHandler<Env>;
