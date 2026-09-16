---
name: service-sharing
description: Run, register and troubleshoot a Web service that a user will access through Zork Mesh.
---

The Agent owns the server process through shell.run or job.register. Use job.register when the requested background service should survive Station restarts. Keep its working directory and port stable, and retain its logs. Inspect existing services and processes before starting a duplicate.

Bind the server to loopback. Register the running server with service.attach; it returns a stable Mesh URL and adds the service to the client’s application list. Registration does not start the process. Use service.list and service.inspect to find an existing registration and check its port; verify application readiness with its actual HTTP endpoint.

Send the returned URL verbatim as a Markdown link with chat.send. The client extracts links into the Chat’s content list. An ordinary external page needs only a Markdown link. Static file deliverables use chat.send attachments.

Build assets, APIs, redirects and WebSocket URLs from the page origin. Do not hardcode the execution Station’s localhost URL into browser-facing code. Development servers must accept the client’s localhost host name.

For failures, read the process’s existing logs on the node that owns them and inspect the registered port. Local filesystem paths belong to that node; shell.run accepts target for another Mesh node. Keep the registered identity while fixing the process, and stop only processes owned by the task. Inspect original invocation results in history before repeating effects that remain uncertain.
