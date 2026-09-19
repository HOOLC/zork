import type { Relay, DiscoveryRecord } from "./index";
import type { Account } from "./account";
import type { LoginAttempt, LoginLimiter } from "./login";

export interface Env {
  RELAY: DurableObjectNamespace<Relay>;
  RECORDS: DurableObjectNamespace<DiscoveryRecord>;
  ACCOUNTS: DurableObjectNamespace<Account>;
  LOGINS: DurableObjectNamespace<LoginAttempt>;
  LOGIN_LIMITS: DurableObjectNamespace<LoginLimiter>;
  PUBLIC_ORIGIN: string;
  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
  AUTH_SIGNING_KEY: string;
  ADMIN_TOKEN?: string;
}
