// Narrow Playwright sidecar. The parent process decides every action.
// This file does not accept JavaScript, a shell, or launch flags from stdin.

import http from "node:http";
import net from "node:net";
import { createInterface } from "node:readline";
import { chromium } from "playwright";

const MAX_TEXT = 4000;
const MAX_ELEMENTS = 40;

let browser;
let context;
let page;
let grants = new Set();
// Form submissions the operator must make; reported with the next observation.
const held = [];
const consoleErrors = [];
const failedRequests = [];

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function originOf(raw) {
  const url = new URL(raw);
  if (url.username || url.password) {
    throw new Error("url credentials are not a grant");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(`scheme ${url.protocol} is not browseable`);
  }
  const port = url.port || (url.protocol === "https:" ? "443" : "80");
  return `${url.protocol}//${url.hostname.toLowerCase()}:${port}`;
}

function allowed(raw) {
  try {
    return grants.has(originOf(raw));
  } catch {
    return false;
  }
}

const ACTIONABLE = new Set(["link", "button", "tab", "textbox", "searchbox", "combobox", "listbox", "checkbox", "radio", "switch", "menuitem", "spinbutton"]);
const TEXT_ROLES = new Set(["textbox", "searchbox", "combobox", "spinbutton"]);

// Roles, names and refs come from Playwright's own aria snapshot, the same
// engine getByRef/getByRole resolve with, so what the model sees is clickable.
async function inputKind(node) {
  if (!TEXT_ROLES.has(node.role)) return "none";
  try {
    const [type, autocomplete] = await page
      .getByRef(node.ref)
      .evaluate((el) => [el.getAttribute("type") || "", el.getAttribute("autocomplete") || ""], null, { timeout: 2000 });
    return type.toLowerCase() === "password" || /password|one-time-code|cc-/i.test(autocomplete) ? "secret" : "text";
  } catch {
    return "secret";
  }
}

function absolute(raw) {
  try {
    return new URL(raw, page.url()).href;
  } catch {
    return null;
  }
}

async function observe() {
  const elements = [];
  const walk = async (nodes) => {
    for (const node of nodes || []) {
      if (elements.length >= MAX_ELEMENTS) return;
      if (typeof node !== "object" || node === null) continue;
      const name = String(node.name || node.placeholder || "").replace(/\s+/g, " ").trim().slice(0, 120);
      if (node.ref && name && ACTIONABLE.has(node.role)) {
        elements.push({
          ref: node.ref,
          role: node.role,
          name,
          href: node.url ? absolute(node.url) : null,
          input: await inputKind(node),
        });
      }
      await walk(node.children);
    }
  };
  await walk(await page.ariaSnapshotJSON({ mode: "ai", timeout: 5000 }));
  const text = (await page.locator("body").innerText()).replace(/\s+/g, " ").trim().slice(0, MAX_TEXT);
  return {
    url: page.url(),
    title: await page.title(),
    text,
    elements,
    held: held.splice(0),
  };
}

function quiet(stream) {
  stream.on("error", () => {});
}

const HOP_BY_HOP = new Set(["connection", "keep-alive", "proxy-connection", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"]);

function forwardable(headers) {
  return Object.fromEntries(Object.entries(headers).filter(([key]) => !HOP_BY_HOP.has(key)));
}

function startProxy() {
  const server = http.createServer((req, res) => {
    quiet(req);
    quiet(res);
    let target;
    try {
      target = new URL(req.url);
    } catch {
      res.writeHead(403);
      res.end();
      return;
    }
    if (!allowed(target.href)) {
      res.writeHead(403);
      res.end();
      return;
    }
    const upstream = http.request(
      {
        hostname: target.hostname,
        port: target.port || 80,
        path: `${target.pathname}${target.search}`,
        method: req.method,
        // Keep cookies, CSRF and content-type so a logged-in console POST works.
        headers: { ...forwardable(req.headers), host: target.host },
      },
      (up) => {
        quiet(up);
        if (!res.headersSent) res.writeHead(up.statusCode || 502, up.headers);
        up.pipe(res);
      },
    );
    quiet(upstream);
    upstream.on("error", () => {
      if (!res.headersSent) res.writeHead(502);
      res.end();
    });
    req.pipe(upstream);
  });
  server.on("clientError", (_err, socket) => {
    quiet(socket);
    socket.end("HTTP/1.1 400 Bad Request\r\n\r\n");
  });
  server.on("connect", (req, clientSocket) => {
    quiet(clientSocket);
    const [host, portText] = String(req.url).split(":");
    const port = Number(portText || 443);
    if (!allowed(`https://${host}:${port}/`)) {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    const remote = net.connect(port, host, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      remote.pipe(clientSocket);
      clientSocket.pipe(remote);
    });
    quiet(remote);
    remote.on("error", () => clientSocket.destroy());
    clientSocket.on("error", () => remote.destroy());
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server.address().port));
  });
}

async function start(message) {
  if (browser) throw new Error("session already started");
  grants = new Set(message.grants || []);
  const proxyPort = await startProxy();
  browser = await chromium.launch({
    channel: "chrome",
    headless: message.headless !== false,
    args: ["--disable-features=WebRtc,TranslateUI"],
  });
  context = await browser.newContext({
    proxy: { server: `http://127.0.0.1:${proxyPort}`, bypass: "<-loopback>" },
    serviceWorkers: "block",
    acceptDownloads: false,
    ignoreHTTPSErrors: false,
  });
  await context.route("**/*", (route) => {
    const request = route.request();
    const url = request.url();
    if (url.startsWith("data:") || url.startsWith("blob:") || url.startsWith("about:")) {
      route.continue();
      return;
    }
    if (!allowed(url)) {
      route.abort("blockedbyclient");
      return;
    }
    // Enter in a text box submits its form just like the submit button the
    // policy hands to the operator, so a non-GET page load is held here.
    if (request.isNavigationRequest() && request.method() !== "GET") {
      if (held.length < 20) held.push(`${request.method()} ${url.slice(0, 200)}`);
      route.abort("blockedbyclient");
      return;
    }
    route.continue();
  });
  await context.routeWebSocket("**/*", (ws) => {
    ws.close();
  });
  page = await context.newPage();
  page.on("console", (msg) => {
    if (msg.type() === "error" && consoleErrors.length < 20) {
      consoleErrors.push(msg.text().slice(0, 300));
    }
  });
  page.on("requestfailed", (req) => {
    if (failedRequests.length < 20) {
      failedRequests.push({ url: req.url().slice(0, 200), error: req.failure()?.errorText || "failed" });
    }
  });
  page.on("download", (download) => {
    download.cancel();
  });
  return { started: true };
}

async function locatorFor(target) {
  if (typeof target?.ref !== "string") throw new Error("stale or unknown target");
  const locator = page.getByRef(target.ref);
  if ((await locator.count().catch(() => 0)) !== 1) throw new Error("stale or unknown target");
  return locator;
}

async function handle(message) {
  switch (message.op) {
    case "start":
      return start(message);
    case "navigate":
      await page.goto(message.url, { waitUntil: "domcontentloaded", timeout: 10000 });
      return observe();
    case "observe":
      return observe();
    case "click":
    case "follow_link": {
      const locator = await locatorFor(message.target);
      await locator.click({ timeout: 5000 });
      await page.waitForLoadState("domcontentloaded", { timeout: 5000 }).catch(() => {});
      return observe();
    }
    case "type": {
      const locator = await locatorFor(message.target);
      await locator.fill(message.text, { timeout: 5000 });
      return observe();
    }
    case "press_enter": {
      const locator = await locatorFor(message.target);
      await locator.press("Enter", { timeout: 5000 });
      await page.waitForLoadState("domcontentloaded", { timeout: 5000 }).catch(() => {});
      return observe();
    }
    case "select_tab": {
      const locator = page.getByRole("tab", { name: message.tab, exact: true });
      if ((await locator.count()) !== 1) throw new Error("unknown tab");
      await locator.click({ timeout: 5000 });
      return observe();
    }
    case "scroll":
      await page.mouse.wheel(0, message.dy || 0);
      return observe();
    case "wait":
      await page.waitForTimeout(message.timeout_ms);
      return observe();
    case "diagnostics":
      return { console_errors: consoleErrors, failed_requests: failedRequests };
    case "close":
      await browser?.close();
      browser = undefined;
      return { closed: true };
    default:
      throw new Error("malformed browser action");
  }
}

const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  if (!line.trim()) continue;
  let message;
  try {
    message = JSON.parse(line);
    const result = await handle(message);
    send({ id: message.id, ok: true, result });
  } catch (error) {
    send({ id: message?.id, ok: false, error: String(error.message || error).slice(0, 300) });
  }
}
// stdin closed: the parent is gone or done. Close Chrome rather than orphan it.
await browser?.close().catch(() => {});
process.exit(0);
