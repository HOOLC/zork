export function escape(value: string) {
  return value.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
// A self POST needs a non-null Origin; only same-origin requests receive the
// referrer. Chromium also applies form-action to the subsequent Google redirect.
export function devicePage(title: string, content: string, cookies?: string): Response {
  return new Response(
    `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escape(title)} · Zork</title><style>body{margin:0;background:#f6f5f2;color:#242424;font:16px/1.7 system-ui}main{max-width:420px;margin:15vh auto;padding:32px}h1{font-size:28px}button{font:inherit;background:#242424;color:white;border:0;border-radius:10px;padding:12px 24px;cursor:pointer}p{margin:20px 0}</style><main><p>Zork</p><h1>${escape(title)}</h1>${content}</main></html>`,
    {
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
        "referrer-policy": "same-origin",
        "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self' https://accounts.google.com; frame-ancestors 'none'",
        ...(cookies ? { "set-cookie": cookies } : {}),
      },
    },
  );
}
