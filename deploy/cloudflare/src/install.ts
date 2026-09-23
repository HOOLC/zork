import { randomSecret } from "./auth";
import { devicePage } from "./page";

/** The invitation stays in the browser fragment and is never sent to GitHub. */
export async function installPage(fetcher: typeof fetch = fetch): Promise<Response> {
  let bootstrap: string | null = null;
  let version: string | null = null;
  try {
    const response = await fetcher("https://api.github.com/repos/HOOLC/zork/releases/latest", {
      headers: { accept: "application/vnd.github+json", "user-agent": "Zork installer" },
    });
    if (response.ok) {
      const release = (await response.json()) as { tag_name?: string; assets?: { name: string }[] };
      if (release.tag_name && /^v\d+\.\d+\.\d+$/.test(release.tag_name)) {
        const candidate = release.tag_name.slice(1);
        const names = new Set(release.assets?.map((asset) => asset.name));
        const required = ["install.sh", "VERSION", "manifest.tsv", "SHA256SUMS", ...["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"].map((platform) => `zork-${candidate}-${platform}.tar.gz`)];
        if (required.every((name) => names.has(name))) {
          version = candidate;
          bootstrap = `https://github.com/HOOLC/zork/releases/download/${release.tag_name}/install.sh`;
        }
      }
    }
  } catch {
    /* An unavailable release never produces a fabricated download command. */
  }
  const nonce = randomSecret();
  const script = `
const parameters = new URLSearchParams(location.hash.slice(1));
const ticket = parameters.get("ticket") || "";
const channel = parameters.get("channel") || "release";
const source = parameters.get("source") || "";
const testVersion = parameters.get("version") || "";
history.replaceState(null, "", location.pathname);
const message = document.getElementById("message");
const bootstrap = ${JSON.stringify(bootstrap)};
if (!/^zj1_[A-Za-z0-9_-]{64,8192}$/.test(ticket) || !["release","dev","test"].includes(channel)) {
  message.textContent = "安装链接无效，请在电脑 Zork 中重新生成。";
} else if (channel === "test") {
  let url;
  try { url = new URL(source); } catch {}
  if (!url || !["https:", "http:"].includes(url.protocol) || url.username || url.password || url.search || url.hash || !/^[0-9]+\\.[0-9]+\\.[0-9]+(?:-[A-Za-z0-9.-]+)?$/.test(testVersion)) {
    message.textContent = "test 安装包来源未配置或无效，请重新打包测试客户端。";
  } else {
    const quote = (value) => "'" + value.replaceAll("'", "'\\\\''") + "'";
    const base = url.href.replace(/\\/$/, "");
    const insecure = url.protocol === "http:" ? " --allow-http-test" : "";
    const command = "curl -fsSL " + quote(base + "/download/v" + testVersion + "/install.sh") + " | sh -s -- --version " + testVersion + " --base-url " + quote(base) + insecure + " -- mesh join '" + ticket + "' --channel test";
    message.textContent = "这是局域网测试安装包，需要连接开发局域网。";
    document.getElementById("command").textContent = command;
    document.getElementById("instructions").hidden = false;
    document.getElementById("copy").onclick = async () => { try { await navigator.clipboard.writeText(command); message.textContent = "安装命令已复制。"; } catch { message.textContent = "请选中下面的命令手动复制。"; } };
  }
} else if (bootstrap) {
  const command = "curl -fsSL '" + bootstrap + "' | sh -s -- --version ${version || ""} -- mesh join '" + ticket + "' --channel " + channel;
  document.getElementById("command").textContent = command;
  document.getElementById("instructions").hidden = false;
  document.getElementById("copy").onclick = async () => { try { await navigator.clipboard.writeText(command); message.textContent = "安装命令已复制。"; } catch { message.textContent = "请选中下面的命令手动复制。"; } };
}
`;
  const response = devicePage(
    "连接执行设备",
    `<p id="message">${bootstrap ? "在这台 macOS 或 Linux 设备的终端执行命令，安装后会加入你的设备。链接短时有效且只能使用一次。" : "当前尚无完整的原生安装包可供下载。请等待新版本发布后，在电脑 Zork 中重新生成安装链接。"}</p><section id="instructions" hidden><pre id="command" style="white-space:pre-wrap;overflow-wrap:anywhere"></pre><button id="copy">复制安装命令</button></section><script nonce="${nonce}">${script}</script>`,
  );
  response.headers.set("referrer-policy", "no-referrer");
  response.headers.set("content-security-policy", `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'`);
  return response;
}
