import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { installPage } from "../src/install.ts";

test("test installation requires its own source and never falls back to public releases", async () => {
  const html = await (await installPage((async () => new Response(null, { status: 404 })) as typeof fetch)).text();
  const script = html.match(/<script[^>]*>([\s\S]*?)<\/script>/)![1];
  const ticket = "zj1_" + "a".repeat(64);
  for (const source of ["", "http://192.168.1.2:19000/zork-test/build", "https://host.invalid/a'b", "javascript:alert(1)"]) {
    const elements: Record<string, { textContent: string; hidden: boolean }> = {};
    const parameters = new URLSearchParams({ ticket, channel: "test", source, version: "1.2.3" });
    vm.runInNewContext(script, {
      URL,
      URLSearchParams,
      location: { hash: "#" + parameters, pathname: "/install" },
      history: { replaceState() {} },
      document: {
        getElementById(id: string) {
          return (elements[id] ||= { textContent: "", hidden: true });
        },
      },
    });
    if (source.startsWith("http")) {
      assert.equal(elements.instructions.hidden, false);
      assert.match(elements.command.textContent, /--base-url/);
      assert.match(elements.command.textContent, /--channel test$/);
      assert.doesNotMatch(elements.command.textContent, /github\.com/);
      if (source.startsWith("http:")) assert.match(elements.command.textContent, /--allow-http-test/);
      if (source.includes("'")) assert.match(elements.command.textContent, /'\\''/);
    } else {
      assert.match(elements.message.textContent, /来源未配置或无效/);
      assert.equal(elements.command, undefined);
    }
  }
});

test("install links do not fabricate a download when no complete release exists", async () => {
  for (const body of [null, { tag_name: "v1.2.3", assets: [{ name: "install.sh" }] }]) {
    const page = await installPage((async () => new Response(JSON.stringify(body), { status: body ? 200 : 404 })) as typeof fetch);
    const html = await page.text();
    assert.match(html, /当前尚无完整的原生安装包/);
    assert.doesNotMatch(html, /releases\/download\/v/);
    assert.equal(page.headers.get("referrer-policy"), "no-referrer");
    assert.match(page.headers.get("content-security-policy")!, /script-src 'nonce-/);
  }
});

test("a complete release pins the bootstrap and consumes invitation credentials only from the fragment", async () => {
  const names = ["install.sh", "VERSION", "manifest.tsv", "SHA256SUMS", ...["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"].map((platform) => `zork-1.2.3-${platform}.tar.gz`)];
  let requested = "";
  const page = await installPage((async (url) => {
    requested = String(url);
    return Response.json({ tag_name: "v1.2.3", assets: names.map((name) => ({ name })) });
  }) as typeof fetch);
  const html = await page.text();
  assert.equal(requested, "https://api.github.com/repos/HOOLC/zork/releases/latest");
  assert.match(html, /releases\/download\/v1\.2\.3\/install\.sh/);
  assert.match(html, /--version 1\.2\.3 -- mesh join/);
  assert.match(html, /location\.hash/);
  assert.match(html, /history\.replaceState/);
});
