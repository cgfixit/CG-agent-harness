// Narrow Playwright sidecar. The parent process decides every action.
// This file does not accept JavaScript, a shell, or launch flags from stdin.

import crypto from "node:crypto";
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
const MAX_FIND_CHARS = 80;
const MAX_NOT_GRANTED = 20;
// Page-controlled strings in a reply. A longer href is dropped, not cut, so a
// cut link is never followed; the parent also bounds the whole reply line.
const MAX_HREF = 8192;
const MAX_URL = 8192;
const MAX_TITLE = 300;
// Bounds a page operation that has no timeout option of its own.
const OP_TIMEOUT_MS = 5000;
// Only this session's Chrome knows it: Playwright answers the proxy's 407 with
// it, and any other local process is refused before the grant is consulted.
const PROXY_USER = "cgah";
const PROXY_SECRET = crypto.randomBytes(24).toString("hex");
const PROXY_AUTH = `Basic ${Buffer.from(`${PROXY_USER}:${PROXY_SECRET}`).toString("base64")}`;

let context;
let page;
// Fresh Chrome profile for this session only; removed when the sidecar exits.
let profileDir;
let grants = new Set();
// Only test-support fixtures set this; production refuses non-public literals.
let allowPrivate = false;
// Form submissions the operator must make; reported with the next observation.
const held = [];
// Origins a page asked for that this task was not granted; reported the same way.
const notGranted = new Set();
// Tabs a page opened whose verdict is pending; every action waits for it.
const opening = new Set();

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
  // browser.rs parse_origin drops one trailing dot too, so both sides name
  // example.com. and example.com as the same grant.
  return `${url.protocol}//${url.hostname.toLowerCase().replace(/\.$/, "")}:${port}`;
}

function allowed(raw) {
  try {
    return grants.has(originOf(raw));
  } catch {
    return false;
  }
}

// Names an origin a page asked for and did not get, for the operator. Only
// the origin: a path could carry page data.
function refuse(raw) {
  try {
    if (notGranted.size < MAX_NOT_GRANTED) notGranted.add(originOf(raw));
  } catch {}
}

const ACTIONABLE = new Set(["link", "button", "tab", "textbox", "searchbox", "combobox", "listbox", "option", "checkbox", "radio", "switch", "menuitem", "spinbutton"]);
const TEXT_ROLES = new Set(["textbox", "searchbox", "combobox", "spinbutton"]);
// Roles whose accessible name is their own text.
const NAMED_BY_CONTENT = new Set(["link", "button", "tab", "option", "checkbox", "radio", "switch", "menuitem"]);

// The snapshot leaves `name` off an element whose label has inline markup,
// such as <a><code>zipfile</code> — ZIP archives</a>: the label is then the
// text of its children.
function contentOf(node) {
  if (typeof node === "string") return node;
  if (typeof node !== "object" || node === null) return "";
  return [node.name, node.text, ...(node.children || []).map(contentOf)].filter(Boolean).join(" ");
}

function nameOf(node) {
  const own = node.name || node.placeholder || (NAMED_BY_CONTENT.has(node.role) ? (node.children || []).map(contentOf).join(" ") : "");
  return String(own).replace(/\s+/g, " ").trim().slice(0, 120);
}

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
    const href = new URL(raw, page.url()).href;
    return href.length <= MAX_HREF ? href : null;
  } catch {
    return null;
  }
}

// The text around each place `find` appears, in page order.
function excerpts(body, find) {
  const lower = body.toLowerCase();
  const parts = [];
  let end = 0;
  for (let at = lower.indexOf(find); at >= 0; at = lower.indexOf(find, Math.max(end, at + 1))) {
    const start = Math.max(end, at - 150);
    end = Math.min(body.length, at + find.length + 250);
    parts.push(body.slice(start, end));
  }
  return parts.join(" … ");
}

// One page of the document: MAX_ELEMENTS elements and MAX_TEXT characters.
// `view.page` picks a later page; `view.find` keeps only the elements whose
// name contains that text, and the page text around it.
async function observe(view = {}) {
  const find = typeof view.find === "string" ? view.find.trim().toLowerCase().slice(0, MAX_FIND_CHARS) : "";
  const pageNo = Number.isInteger(view.page) && view.page > 0 ? view.page : 1;
  const elements = [];
  let matched = 0;
  const optionBudget = { left: MAX_OPTION_CHARS };
  const walk = async (nodes) => {
    for (const node of nodes || []) {
      if (typeof node !== "object" || node === null) continue;
      const name = ACTIONABLE.has(node.role) ? nameOf(node) : "";
      if (node.ref && name && name.toLowerCase().includes(find)) {
        matched += 1;
        if (matched > (pageNo - 1) * MAX_ELEMENTS && elements.length < MAX_ELEMENTS) {
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
      }
      await walk(node.children);
    }
  };
  await walk(await page.ariaSnapshotJSON({ mode: "ai", timeout: 5000 }));
  const body = (await page.locator("body").innerText({ timeout: 5000 })).replace(/\s+/g, " ").trim();
  const shown = find ? excerpts(body, find) : body;
  const result = {
    url: page.url().slice(0, MAX_URL),
    title: (await bounded(page.title(), "title")).slice(0, MAX_TITLE),
    text: shown.slice((pageNo - 1) * MAX_TEXT, pageNo * MAX_TEXT),
    elements,
    page: pageNo,
    pages: Math.max(1, Math.ceil(matched / MAX_ELEMENTS), Math.ceil(shown.length / MAX_TEXT)),
    held: held.splice(0),
    not_granted: [...notGranted],
  };
  notGranted.clear();
  return result;
}

function quiet(stream) {
  stream.on("error", () => {});
}

// A stuck renderer must fail one call, not hold the task until its deadline.
function bounded(promise, what) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} timed out`)), OP_TIMEOUT_MS);
  });
  return Promise.race([promise, expired]).finally(() => clearTimeout(timer));
}

function authorized(req) {
  const given = Buffer.from(String(req.headers["proxy-authorization"] || ""));
  const want = Buffer.from(PROXY_AUTH);
  return given.length === want.length && crypto.timingSafeEqual(given, want);
}

const HOP_BY_HOP = new Set(["connection", "keep-alive", "proxy-connection", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"]);

function forwardable(headers) {
  return Object.fromEntries(Object.entries(headers).filter(([key]) => !HOP_BY_HOP.has(key)));
}

function startProxy() {
  const server = http.createServer((req, res) => {
    quiet(req);
    quiet(res);
    if (!authorized(req)) {
      res.writeHead(407, { "proxy-authenticate": 'Basic realm="cgah"' });
      res.end();
      return;
    }
    let target;
    try {
      target = new URL(req.url);
    } catch {
      res.writeHead(403);
      res.end();
      return;
    }
    // Plain http only: https arrives as CONNECT. An absolute-form https:// URL
    // here used to throw inside http.request and kill the sidecar.
    if (target.protocol !== "http:") {
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
    let upstream;
    try {
      upstream = http.request(
        {
          // urlToHttpOptions strips IPv6 brackets, which a raw hostname keeps.
          ...options,
          method: req.method,
          // Keep cookies, CSRF and content-type so a logged-in console POST works.
          // forwardable drops Proxy-Authorization, so the session secret stays here.
          headers: { ...forwardable(req.headers), host: target.host },
          lookup: publicLookup,
        },
        (up) => {
          quiet(up);
          if (!res.headersSent) res.writeHead(up.statusCode || 502, up.headers);
          up.pipe(res);
        },
      );
    } catch {
      res.writeHead(502);
      res.end();
      return;
    }
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
    if (!authorized(req)) {
      clientSocket.end('HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm="cgah"\r\n\r\n');
      return;
    }
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

// One tab per session. A tab opened by a target=_blank link or a script's
// window.open becomes the current one, and the old one closes, only once it
// shows a granted page. Any other new tab (an ad, a sign-in on another site)
// is closed and the task keeps its page. Grants and holds belong to the
// context, so a new tab reaches nothing the old one could not.
async function adopt(opened) {
  opened.on("download", (download) => {
    download.cancel();
  });
  // The same budget as navigate; a tab still blank after it is not adopted.
  await opened.waitForLoadState("domcontentloaded", { timeout: 10000 }).catch(() => {});
  if (opened.isClosed() || !allowed(opened.url())) {
    await opened.close().catch(() => {});
    return;
  }
  const previous = page;
  page = opened;
  await previous.close().catch(() => {});
}

function track(opened) {
  const verdict = adopt(opened).finally(() => opening.delete(verdict));
  opening.add(verdict);
}

// Records a request that could write to a server, for the operator. It was not sent.
function hold(method, url) {
  if (held.length < 20) held.push(`${method} ${url.slice(0, 200)}`);
}

// Every document a task loads carries this policy beside the site's own, and
// policies only narrow, so no page can start a dedicated or shared worker.
// Workers are where the per-page guards stop: context.route never sees a
// SharedWorker's requests, and routeWebSocket is a page script that a worker's
// WebSocket never meets. Blob, data and about:blank documents inherit it.
const WORKER_POLICY = { name: "Content-Security-Policy", value: "worker-src 'none'" };
// The body comes back decoded, so these would describe bytes that are gone.
const STALE_BODY_HEADERS = new Set(["content-encoding", "content-length", "transfer-encoding"]);

// The browser-wide network gate under context.route. Chrome pauses here every
// request any page or worker makes, so it holds what the page route never saw,
// and it adds WORKER_POLICY to every document response. Anything that fails
// in here fails the request: nothing passes ungated.
async function gate(cdp, event) {
  const { requestId, request } = event;
  if (event.responseErrorReason !== undefined) {
    return cdp.send("Fetch.failRequest", { requestId, errorReason: event.responseErrorReason });
  }
  if (event.responseStatusCode !== undefined) {
    // Response stage: documents only (see the Fetch.enable patterns). A redirect
    // has no body to read and renders nothing; its target is a new request that
    // pauses here again, so it meets the grant check and this policy then.
    const status = event.responseStatusCode;
    const redirect = status >= 300 && status < 400 && (event.responseHeaders || []).some(({ name }) => name.toLowerCase() === "location");
    if (redirect) return cdp.send("Fetch.continueRequest", { requestId });
    const { body, base64Encoded } = await cdp.send("Fetch.getResponseBody", { requestId });
    const headers = (event.responseHeaders || []).filter(({ name }) => !STALE_BODY_HEADERS.has(name.toLowerCase()));
    return cdp.send("Fetch.fulfillRequest", {
      requestId,
      responseCode: event.responseStatusCode,
      responsePhrase: event.responseStatusText || undefined,
      responseHeaders: [...headers, WORKER_POLICY],
      body: base64Encoded ? body : Buffer.from(body).toString("base64"),
    });
  }
  const local = /^(data|blob|about):/.test(request.url);
  // The proxy refuses an ungranted origin too; the page route names it.
  if (!local && !allowed(request.url)) {
    return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    hold(request.method, request.url);
    // A held page load answers 204 so the page stays, as the page route does.
    if (event.resourceType === "Document") {
      return cdp.send("Fetch.fulfillRequest", { requestId, responseCode: 204, body: "" });
    }
    return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
  }
  return cdp.send("Fetch.continueRequest", { requestId });
}

async function startNetworkGate() {
  const cdp = await context.browser().newBrowserCDPSession();
  cdp.on("Fetch.requestPaused", (event) => {
    gate(cdp, event).catch(() =>
      cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "BlockedByClient" }).catch(() => {}),
    );
  });
  await cdp.send("Fetch.enable", {
    patterns: [
      { urlPattern: "*", requestStage: "Request" },
      { urlPattern: "*", requestStage: "Response", resourceType: "Document" },
    ],
  });
}

async function start(message) {
  if (context) throw new Error("session already started");
  grants = new Set(message.grants || []);
  allowPrivate = message.allow_private === true;
  const proxyPort = await startProxy();
  profileDir = freshProfile();
  context = await chromium.launchPersistentContext(profileDir, {
    channel: "chrome",
    // Without this Playwright adds --no-sandbox, and hostile pages render unsandboxed.
    chromiumSandbox: true,
    headless: message.headless !== false,
    args: ["--disable-features=TranslateUI"],
    proxy: { server: `http://127.0.0.1:${proxyPort}`, bypass: "<-loopback>", username: PROXY_USER, password: PROXY_SECRET },
    serviceWorkers: "block",
    acceptDownloads: false,
    ignoreHTTPSErrors: false,
  });
  // Before any page loads: the first document must already carry WORKER_POLICY.
  await startNetworkGate();
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
      hold(method, url);
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
  page = context.pages()[0] ?? (await context.newPage());
  page.on("download", (download) => {
    download.cancel();
  });
  context.on("page", track);
  // Page requests only, so Chrome's own background traffic (also refused by
  // the proxy) is not listed. A redirect to another site fails here too.
  context.on("requestfailed", (request) => {
    if (!allowed(request.url())) refuse(request.url());
  });
  // The port is loopback-only and useless without this session's credential;
  // the host's guard fixture probes it unauthenticated (#945).
  return { started: true, proxy_port: proxyPort };
}

// Wait for the page to load, script content to render, held requests to
// register and any tab the page opened to be adopted or closed.
async function settle() {
  for (let pass = 0; pass < 2; pass += 1) {
    await page.waitForLoadState("domcontentloaded", { timeout: 5000 }).catch(() => {});
    await page.waitForLoadState("networkidle", { timeout: 2000 }).catch(() => {});
    if (!opening.size) return;
    await Promise.allSettled([...opening]);
  }
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
  if (opening.size) await Promise.allSettled([...opening]);
  switch (message.op) {
    case "start":
      return start(message);
    case "navigate":
      await page.goto(message.url, { waitUntil: "domcontentloaded", timeout: 10000 });
      // A page that renders from a fetch is still a shell at domcontentloaded.
      await settle();
      return observe();
    case "observe":
      return observe(message);
    case "click":
    case "follow_link": {
      const locator = await locatorFor(message.target);
      const tab = (await opensTab(locator)) ? page.waitForEvent("popup", { timeout: 5000 }).catch(() => null) : null;
      await locator.click({ timeout: 5000 });
      await tab;
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
      // Native <select> only. Observations list labels cut to 80 characters,
      // so match the listed text, not only the full label: an exact label
      // first, then the one option whose cut label equals it.
      const locator = await locatorFor(message.target);
      const index = await locator.evaluate(
        (el, wanted) => {
          const labels = [...(el.options || [])].map((option) => (option.label || option.text).replace(/\s+/g, " ").trim());
          const one = (hits) => (hits.length === 1 ? hits[0] : -1);
          const exact = one(labels.flatMap((label, i) => (label === wanted ? [i] : [])));
          return exact >= 0 ? exact : one(labels.flatMap((label, i) => (label.slice(0, 80) === wanted ? [i] : [])));
        },
        String(message.value ?? ""),
        { timeout: 2000 },
      );
      if (index < 0) throw new Error("no single listed option has that label");
      await locator.selectOption({ index }, { timeout: 5000 });
      await settle();
      return observe();
    }
    case "select_tab": {
      const locator = page.getByRole("tab", { name: message.tab, exact: true });
      if ((await bounded(locator.count(), "tab lookup")) !== 1) throw new Error("unknown tab");
      await locator.click({ timeout: 5000 });
      return observe();
    }
    case "scroll":
      await bounded(page.mouse.wheel(0, message.dy || 0), "scroll");
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
