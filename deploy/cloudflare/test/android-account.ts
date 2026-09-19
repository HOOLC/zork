// Actual Android settings UI and JNI/core, with only Google replaced by a signed fixture.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import net from "node:net";
import { spawn, execFile as execCallback } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { ulid } from "ulid";
import { harness } from "./harness.ts";
const exec = promisify(execCallback);
const [apkArg, serial, outputArg] = process.argv.slice(2);
if (!apkArg || !serial?.startsWith("emulator-") || !outputArg) throw new Error("usage: android-account.ts APK ISOLATED_EMULATOR REPORT_DIR");
const sourceApk = path.resolve(apkArg),
  output = path.resolve(outputArg),
  pkg = "ing.zork.android.debug";
const adb = process.env.ADB ?? path.join(process.env.HOME!, "Library/Android/sdk/platform-tools/adb");
await fs.mkdir(output, { recursive: true });
const apk = path.join(output, "zork-account-tested.apk");
await fs.copyFile(sourceApk, apk);
const call = (...args: string[]) => exec(adb, ["-s", serial, ...args], { timeout: 30000, maxBuffer: 4 * 1024 * 1024 });
async function until(check: () => Promise<boolean>, label: string) {
  const deadline = Date.now() + 60000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error("timeout: " + label);
}
async function ui() {
  await call("shell", "uiautomator", "dump", "/sdcard/zork-account-window.xml");
  return (await call("shell", "cat", "/sdcard/zork-account-window.xml")).stdout;
}
async function click(text: string) {
  let bounds: RegExpMatchArray | null = null;
  await until(async () => {
    const xml = await ui();
    const tag = xml.match(/<node\s[^>]+>/g)?.find((s) => s.includes(`text="${text}"`) || s.includes(`content-desc="${text}"`));
    bounds = tag?.match(/bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/) ?? null;
    return !!bounds;
  }, "UI: " + text);
  const b = bounds!;
  await call("shell", "input", "tap", String((Number(b[1]) + Number(b[3])) / 2), String((Number(b[2]) + Number(b[4])) / 2));
}
async function screenshot(name: string) {
  const image = await exec(adb, ["-s", serial, "exec-out", "screencap", "-p"], { encoding: "buffer", timeout: 10000, maxBuffer: 8 * 1024 * 1024 });
  await fs.writeFile(path.join(output, name + ".png"), image.stdout);
}
async function account() {
  try {
    return JSON.parse((await call("shell", "run-as", pkg, "cat", "no_backup/client/account/relay.json")).stdout);
  } catch {
    return null;
  }
}
const server = net.createServer();
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = (server.address() as net.AddressInfo).port;
await new Promise<void>((resolve) => server.close(() => resolve()));
const h = await harness({ origin: `http://127.0.0.1:${port}`, port });
const checks: string[] = [];
function pass(label: string) {
  checks.push(label);
  console.log("PASS: " + label);
}
try {
  await call("install", "-r", apk);
  await call("shell", "am", "force-stop", pkg);
  await call("shell", "run-as", pkg, "mkdir", "-p", "no_backup/client");
  const write = spawn(adb, ["-s", serial, "shell", "run-as", pkg, "tee", "no_backup/client/services.json"], { stdio: ["pipe", "ignore", "pipe"] });
  write.stderr.resume();
  write.stdin.end(JSON.stringify({ relay_urls: [h.origin], discovery_url: h.origin + "/pkarr" }));
  await new Promise<void>((resolve, reject) => write.on("exit", (code) => (code === 0 ? resolve() : reject(new Error("test services write failed")))));
  await call("reverse", `tcp:${port}`, `tcp:${port}`);
  await call("shell", "am", "start", "-W", "-n", pkg + "/ing.zork.android.MainActivity");
  await click("设置");
  await click("Zork 账号");
  await screenshot("signed-out");
  await click("使用 Google 登录");
  let requestUrl = "";
  await until(async () => {
    const requests = (await (await h.fetch("/__test/devices")).json()) as string[];
    requestUrl = requests.at(-1) ?? "";
    return !!requestUrl;
  }, "native device authorization");
  const url = new URL(requestUrl);
  const page = await h.fetch(url.pathname);
  const cookie = page.headers.get("set-cookie")!.split(";")[0];
  const csrf = /name="csrf" value="([^"]+)"/.exec(await page.text())![1];
  const consent = await h.fetch(url.pathname, { method: "POST", redirect: "manual", headers: { origin: h.origin, cookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ csrf }).toString() });
  assert.equal(consent.status, 302);
  const google = new URL(consent.headers.get("location")!),
    code = ulid();
  h.codes.set(code, { nonce: google.searchParams.get("nonce")!, challenge: google.searchParams.get("code_challenge")!, sub: "android-google-user" });
  assert.equal((await h.fetch("/v1/auth/google/callback?" + new URLSearchParams({ state: google.searchParams.get("state")!, code }), { redirect: "manual", headers: { cookie } })).status, 302);
  await until(async () => !!(await account())?.current, "Google approval received by background core");
  const session = (await account()).current;
  pass("Android account login works before any Mesh membership and survives browser backgrounding");
  await call("shell", "am", "start", "-W", "-n", pkg + "/ing.zork.android.MainActivity");
  await until(async () => (await ui()).includes("退出 Zork 账号"), "signed-in presentation");
  await screenshot("signed-in");
  await click("退出 Zork 账号");
  await until(async () => {
    const file = await account();
    return !!file && file.current === null && file.pending_revocations.length === 0;
  }, "logout persisted and confirmed");
  assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + session.token } })).status, 401);
  await until(async () => (await ui()).includes("使用 Google 登录"), "signed-out presentation");
  pass("Android logout is reflected through the JNI subscription and revokes the server session");
  await fs.writeFile(
    path.join(output, "result.json"),
    JSON.stringify(
      {
        passed: true,
        checks,
        apk_sha256: createHash("sha256")
          .update(await fs.readFile(apk))
          .digest("hex"),
      },
      null,
      2,
    ),
  );
} catch (error) {
  await screenshot("failure").catch(() => {});
  await fs.writeFile(path.join(output, "result.json"), JSON.stringify({ passed: false, checks, error: error instanceof Error ? error.message : "test failed" }, null, 2));
  throw error;
} finally {
  await call("shell", "am", "force-stop", pkg).catch(() => {});
  await call("reverse", "--remove", `tcp:${port}`).catch(() => {});
  await h.close();
}
