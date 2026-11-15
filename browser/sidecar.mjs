// Narrow Playwright sidecar. The parent process decides every action.
// This file does not accept JavaScript, a shell, or launch flags from stdin.

import dns from "node:dns";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { urlToHttpOptions } from "node:url";
import { createInterface } from "node:readline";
import { chromium } from "playwright";

const MAX_TEXT = 4000;
const MAX_ELEMENTS = 40;
const MAX_OPTIONS = 25;
// All option labels in one observation, so long country lists stay out of the window.
const MAX_OPTION_CHARS = 1500;

let context;
let page;
// Fresh Chrome profile for this session only; removed when the sidecar exits.
let profileDir;
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

const ACTIONABLE = new Set(["link", "button", "tab", "textbox", "searchbox", "combobox", "listbox", "option", "checkbox", "radio", "switch", "menuitem", "spinbutton"]);
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

// A native <select> lists its options without refs: they are chosen by label
// with select, not clicked. Options with a ref (ARIA listboxes) are elements.
function optionNames(node, budget, out = []) {
  for (const child of node.children || []) {
    if (out.length >= MAX_OPTIONS || budget.left <= 0) break;
    if (typeof child !== "object" || child === null || child.ref) continue;
    if (child.role === "option" && child.name) {
      const label = String(child.name).replace(/\s+/g, " ").trim().slice(0, 80);
      budget.left -= label.length;
      out.push(label);
    } else optionNames(child, budget, out);
  }
  return out;
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
  const optionBudget = { left: MAX_OPTION_CHARS };
  const walk = async (nodes) => {
    for (const node of nodes || []) {
      if (elements.length >= MAX_ELEMENTS) return;
      if (typeof node !== "object" || node === null) continue;
      const name = String(node.name || node.placeholder || "").replace(/\s+/g, " ").trim().slice(0, 120);
      if (node.ref && name && ACTIONABLE.has(node.role)) {
        const element = {
          ref: node.ref,
          role: node.role,
          name,
          href: node.url ? absolute(node.url) : null,
          input: await inputKind(node),
        };
        const options = node.role === "combobox" || node.role === "listbox" ? optionNames(node, optionBudget) : [];
        if (options.length) element.options = options;
        elements.push(element);
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

// A task killed at its deadline cannot remove its profile, which holds that
// task's cookies and cache. Remove profiles whose sidecar process is gone.
function sweepProfiles() {
  let names = [];
  try {
    names = fs.readdirSync(os.tmpdir());
  } catch {}
  for (const name of names) {
    const owner = /^cgah-browser-(\d+)-/.exec(name);
    if (!owner) continue;
    try {
      process.kill(Number(owner[1]), 0);
      continue;
    } catch (error) {
      if (error.code !== "ESRCH") continue;
    }
    try {
      fs.rmSync(path.join(os.tmpdir(), name), { recursive: true, force: true });
    } catch {}
  }
}

// WebRTC sends UDP straight from Chrome, so the proxy never sees it: a page's
// RTCPeerConnection could reach any address and port. This preference keeps
// WebRTC to proxied TCP, where CONNECT meets the grants. Chrome 153 ignored
// the --force-webrtc-ip-handling-policy switch, so the profile carries it.
function freshProfile() {
  sweepProfiles();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `cgah-browser-${process.pid}-`));
  fs.mkdirSync(path.join(dir, "Default"));
  fs.writeFileSync(path.join(dir, "Default", "Preferences"), JSON.stringify({ webrtc: { ip_handling_policy: "disable_non_proxied_udp" } }));
  return dir;
}

// One tab per session. A target=_blank link or a script's window.open opens
// a page that becomes the current one, and the old one closes. Grants and
// holds belong to the context, so the new page reaches nothing the old could not.
function adopt(opened) {
  if (opened === page) return;
  const previous = page;
  page = opened;
  opened.on("download", (download) => {
    download.cancel();
  });
  previous?.close().catch(() => {});
}

async function start(message) {
  if (context) throw new Error("session already started");
  grants = new Set(message.grants || []);
  allowPrivate = message.allow_private === true;
  const proxyPort = await startProxy();
  profileDir = freshProfile();
  context = await chromium.launchPersistentContext(profileDir, {
    channel: "chrome",
    headless: message.headless !== false,
    args: ["--disable-features=TranslateUI"],
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
  adopt(context.pages()[0] ?? (await context.newPage()));
  context.on("page", adopt);
  return { started: true };
}

// Wait for the page to load, script menus to render and held requests to register.
async function settle() {
  await page.waitForLoadState("domcontentloaded", { timeout: 5000 }).catch(() => {});
  await page.waitForLoadState("networkidle", { timeout: 2000 }).catch(() => {});
}

// True when activating the element opens a new tab, so the action waits for it.
async function opensTab(locator) {
  return locator
    .evaluate((el) => {
      const link = el.closest("a[href], area[href]");
      return Boolean(link?.target) && !["_self", "_parent", "_top"].includes(link.target.toLowerCase());
    }, null, { timeout: 2000 })
    .catch(() => false);
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
      const tab = (await opensTab(locator)) ? page.waitForEvent("popup", { timeout: 5000 }).catch(() => null) : null;
      await locator.click({ timeout: 5000 });
      const opened = await tab;
      if (opened) adopt(opened);
      await settle();
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
      await settle();
      return observe();
    }
    case "select": {
      // Native <select> only, by the visible option label the observation listed.
      const locator = await locatorFor(message.target);
      await locator.selectOption({ label: String(message.value ?? "") }, { timeout: 5000 });
      await settle();
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
await context?.close().catch(() => {});
if (profileDir) fs.rmSync(profileDir, { recursive: true, force: true });
process.exit(0);
