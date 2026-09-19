import net from "node:net";
import { spawn } from "node:child_process";
import path from "node:path";
import { harness } from "./harness.ts";
const server = net.createServer();
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = (server.address() as net.AddressInfo).port;
await new Promise<void>((resolve) => server.close(() => resolve()));
const h = await harness({ origin: `http://127.0.0.1:${port}`, port });
try {
  const child = spawn("python3", ["test/device-browser.py", h.origin, path.resolve(process.argv[2])], { stdio: "inherit" });
  process.exitCode = await new Promise<number>((resolve) => child.on("exit", (code) => resolve(code ?? 1)));
} finally {
  await h.close();
}
