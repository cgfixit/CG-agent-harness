// Narrow Playwright sidecar. The parent process decides every action.
// This file does not accept JavaScript, a shell, or launch flags from stdin.

import dns from "node:dns";
import http from "node:http";
import net from "node:net";
import { urlToHttpOptions } from "node:url";
import { createInterface } from "node:readline";
import { chromium } from "playwright";

const MAX_TEXT = 4000;
const MAX_ELEMENTS = 40;

let browser;
let context;
let page;
let grants = new Set();
// Only test-support fixtures set this; production refuses non-public literals.
let allowPrivate = false;
// Form submissions the operator must make; reported with the next observation.
const held = [];

// Same rule as the harness web reader (web_policy::is_public_ip): a granted
// name must resolve to global unicast only. An IP-literal grant skips lookup.
// Two lists: BlockList also matches an IPv4 address against IPv6 rules.
const NON_PUBLIC_V4 = new net.BlockList();
const NON_PUBLIC_V6 = new net.BlockList();
for (const [addr, bits] of [["0.0.0.0", 8], ["10.0.0.0", 8], ["100.64.0.0", 10], ["127.0.0.0", 8], ["169.254.0.0", 16], ["172.16.0.0", 12], ["192.0.0.0", 24], ["192.0.2.0", 24], ["192.88.99.0", 24], ["192.168.0.0", 16], ["198.18.0.0", 15], ["198.51.100.0", 24], ["203.0.113.0", 24], ["224.0.0.0", 3]]) {
  NON_PUBLIC_V4.addSubnet(addr, bits, "ipv4");
}
for (const [addr, bits] of [["::", 3], ["4000::", 2], ["8000::", 1], ["2001::", 23], ["2001:db8::", 32], ["2002::", 16], ["3fff::", 20]]) {
  NON_PUBLIC_V6.addSubnet(addr, bits, "ipv6");
}

// An IP-literal grant skips DNS, so publicLookup never sees it; check it here.
function literalRefused(host) {
  const family = net.isIP(host);
  if (!family || allowPrivate) return false;
  return family === 6 ? NON_PUBLIC_V6.check(host, "ipv6") : NON_PUBLIC_V4.check(host, "ipv4");
}

function publicLookup(hostname, options, callback) {
  dns.lookup(hostname, { ...options, all: true }, (error, addresses) => {
    if (error) return callback(error);
    if (!addresses.length || addresses.some((a) => (a.family === 6 ? NON_PUBLIC_V6.check(a.address, "ipv6") : NON_PUBLIC_V4.check(a.address, "ipv4")))) {
      return callback(Object.assign(new Error(`${hostname} resolved to a non-public address`), { code: "ENOTPUBLIC" }));
    }
    if (options?.all) return callback(null, addresses);
    return callback(null, addresses[0].address, addresses[0].family);
  });
}

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
    const options = urlToHttpOptions(target);
    if (!allowed(target.href) || literalRefused(options.hostname)) {
      res.writeHead(403);
      res.end();
      return;
    }
    const upstream = http.request(
      {
        // urlToHttpOptions strips IPv6 brackets, which a raw hostname keeps.
        ...options,
        method: req.method,
        // Keep cookies, CSRF and content-type so a logged-in console POST works.
        headers: { ...forwardable(req.headers), host: target.host },
        lookup: publicLookup,
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
    let target;
    try {
      target = new URL(`https://${req.url}/`);
    } catch {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    if (!allowed(target.href)) {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    const host = target.hostname.replace(/^\[|\]$/g, "");
    if (literalRefused(host)) {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    const remote = net.connect({ host, port: Number(target.port || 443), lookup: publicLookup }, () => {
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
  allowPrivate = message.allow_private === true;
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
    // Clicks and Enter may run, so anything that could write to a server
    // waits for the operator: every request but GET and HEAD is held here.
    const method = request.method();
    if (method !== "GET" && method !== "HEAD") {
      if (held.length < 20) held.push(`${method} ${url.slice(0, 200)}`);
      // A held page load answers 204 so the page stays; a fetch/XHR fails.
      if (request.isNavigationRequest()) route.fulfill({ status: 204, body: "" });
      else route.abort("blockedbyclient");
      return;
    }
    route.continue();
  });
  await context.routeWebSocket("**/*", (ws) => {
    ws.close();
  });
  page = await context.newPage();
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
      // Let script-driven menus render and any held request register first.
      await page.waitForLoadState("networkidle", { timeout: 2000 }).catch(() => {});
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
