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
// Only the test-support takeover fixture sets this: an approved request
// leaves the page route with a body other than the one the operator was
// shown, so the network gate must refuse it (#1123). It can only make an
// approved request fail.
let rewriteApproved = false;
// Requests only the operator may send: a page load, or one the model's own
// action caused. Reported with the next observation; they end the task.
const held = [];
// Requests a page sent on its own (autosave, analytics). Never sent either,
// but listed and the task goes on (#894).
const heldBackground = [];
// Ops that put the model's input into the page.
const ACTS = new Set(["click", "follow_link", "type", "press_enter", "select", "select_tab", "scroll"]);
// True while one of ACTS runs, until it loads a new page: that page's own
// requests are not the model's.
let acting = false;
// Ops whose effect can be a form the page submits a moment later: a token
// fetch first, or a "Processing…" delay.
const SUBMITS = new Set(["click", "follow_link", "press_enter"]);
// Once one of SUBMITS has returned on the page it acted on, with nothing put
// to the operator, the first POST page load that page sends within this long
// is still that action's step. `performance.now()` deadline; zero is closed.
const LATE_SUBMIT_MS = 5000;
let lateSubmitUntil = 0;
// Origins a page asked for that this task was not granted; reported the same way.
const notGranted = new Set();
// alert, confirm and prompt text since the last observation. Each was
// dismissed, as before; the text is page-written and reported, not obeyed.
const dialogs = [];
const MAX_DIALOGS = 5;
const MAX_DIALOG_CHARS = 300;
// One screen is about 800 px; the policy kernel refuses more than 2,000.
const MAX_SCROLL_PX = 2000;
const MAX_WAIT_MS = 5000;
// Tabs a page opened whose verdict is pending; every action waits for it.
const opening = new Set();
// Longest wait for the parent's verdict on one URL; none in time is a refusal.
const CHECK_TIMEOUT_MS = 5000;
// Questions to the parent that have no verdict yet, by number.
const asked = new Map();
let questions = 0;
// Takeover (#995). Zero means off: every held request is answered at once.
// Otherwise a request held as the operator's step stays paused this long for
// the parent's verdict. One request waits at a time; a second is held.
let takeoverWaitMs = 0;
const MAX_TAKEOVER_WAIT_MS = 600000;
// Resolves when the paused request has been decided and answered.
let pendingApproval = null;
// True once a request of the running op has been paused for the operator.
let askedThisOp = false;
// Answers the open question no, as its wait running out would.
let withdrawQuestion = null;
// True when the parent reads `withdrawn` lines (its `start` says so). An
// older parent takes any line it does not know for the reply it is waiting on.
let reportWithdrawn = false;
// One op's questions share one wait. The parent allows each op its own time
// plus takeover_wait_ms, once; a click whose page sends one request after
// another would otherwise wait that long for each and outlast the parent,
// which then stops the session. So a later question of the same op is asked
// with the time the earlier ones left, and with under a second left the
// request is held unasked. Between ops a question has the whole wait.
// `opStartedAt` is zero between ops.
let opStartedAt = 0;
let opWaitedMs = 0;
const MIN_QUESTION_MS = 1000;
// Requests asked about, open or decided in the last ALLOWANCE_MS:
// {method, url, listed, until, withdraw}. A page that closes can let its
// paused request reach the network gate undecided; the gate matches it
// here, so the question ends and the request is listed once.
const questioned = [];
// One-shot passes for the network gate: {method, url, sha256, until}. The
// page route asks; only a matching request, once, passes the gate.
const allowances = [];
// How long the released request has to reach the gate before its pass lapses.
const ALLOWANCE_MS = 5000;
// Characters of a request body the operator is shown, as text.
const MAX_PREVIEW_CHARS = 2000;
// Requests sent with the operator's approval since the last observation.
const approved = [];
// Requests the operator's block list refused since the last observation.
let blocked = 0;
// The same, for page loads only, so a refused navigate can say why.
let blockedLoads = 0;
// Documents the network gate refused at MAX_DOCUMENT_BYTES.
let oversized = 0;
// Set while navigate runs: called when a top-level page load is held or
// paused for the operator. A form the page submits while it is still parsing
// leaves goto waiting for a domcontentloaded that never comes.
let loadStopped = null;

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

// Every `.slice` cut counts UTF-16 units, so it can split an emoji and leave
// a lone surrogate; JSON.stringify writes that as `\udXXX`, which the host's
// serde_json refuses. U+FFFD keeps each line parseable.
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
function wellFormed(_key, value) {
  return typeof value === "string" ? value.replace(LONE_SURROGATE, "\uFFFD") : value;
}

function send(message) {
  process.stdout.write(`${JSON.stringify(message, wellFormed)}\n`);
}

// Asks the parent whether the operator's block list lets this URL be
// requested. The rules and their matcher live in the parent only; this file
// has no copy. The parent judges a URL over its own 2,048-byte limit by host
// alone, so a cut one gets the same verdict. Anything but a clear yes in time
// is a refusal.
function permitted(url) {
  return new Promise((resolve) => {
    const id = (questions += 1);
    const timer = setTimeout(() => {
      asked.delete(id);
      resolve(false);
    }, CHECK_TIMEOUT_MS);
    asked.set(id, (allow) => {
      clearTimeout(timer);
      asked.delete(id);
      resolve(allow === true);
    });
    send({ check: id, url: url.slice(0, MAX_URL) });
  });
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
async function inputKind(ref) {
  try {
    const [type, autocomplete] = await page
      .getByRef(ref)
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

// Lowercase for matching, one UTF-16 unit per unit, so an index into the
// result is an index into `text`. `toLowerCase` alone turns "İ" (U+0130)
// into two units, the only such mapping it has, and an excerpt cut by those
// indices started one place late for every "İ" before the match.
function fold(text) {
  return text.replaceAll("\u0130", "i").toLowerCase();
}

// The text around each place `find` appears, in page order.
function excerpts(body, find) {
  const lower = fold(body);
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
  // A request this action caused may be waiting on the operator. The
  // observation is of the page once that request has been sent or held.
  while (pendingApproval) {
    await pendingApproval;
    await settle();
  }
  const find = typeof view.find === "string" ? fold(view.find.trim()).slice(0, MAX_FIND_CHARS) : "";
  const pageNo = Number.isInteger(view.page) && view.page > 0 ? view.page : 1;
  const elements = [];
  const inputChecks = [];
  let matched = 0;
  const optionBudget = { left: MAX_OPTION_CHARS };
  const walk = async (nodes) => {
    for (const node of nodes || []) {
      if (typeof node !== "object" || node === null) continue;
      const name = ACTIONABLE.has(node.role) ? nameOf(node) : "";
      if (node.ref && name && fold(name).includes(find)) {
        matched += 1;
        if (matched > (pageNo - 1) * MAX_ELEMENTS && elements.length < MAX_ELEMENTS) {
          const element = {
            ref: node.ref,
            role: node.role,
            name,
            href: node.url ? absolute(node.url) : null,
            input: "none",
          };
          if (TEXT_ROLES.has(node.role)) {
            inputChecks.push(inputKind(node.ref).then((input) => { element.input = input; }));
          }
          const options = node.role === "combobox" || node.role === "listbox" ? optionNames(node, optionBudget) : [];
          if (options.length) element.options = options;
          elements.push(element);
        }
      }
      await walk(node.children);
    }
  };
  await walk(await page.ariaSnapshotJSON({ mode: "ai", timeout: 5000 }));
  await Promise.all(inputChecks);
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
    held_background: heldBackground.splice(0),
    approved: approved.splice(0),
    dialogs: dialogs.splice(0),
    not_granted: [...notGranted],
    blocked,
  };
  notGranted.clear();
  blocked = 0;
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
// The server does the same at start (browser_host::sweep_stale_profiles).
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

// True while `keepPage` opens its own blank tab, which is not a page's popup.
let openingOwnTab = false;

function track(opened) {
  if (openingOwnTab) {
    openingOwnTab = false;
    return;
  }
  const verdict = adopt(opened).finally(() => opening.delete(verdict));
  opening.add(verdict);
}

// A tab can close itself: one a link opened may call window.close(), as
// sign-in, print and "you can close this window" pages do. Once adopted it
// was the session's only tab, so every later action would fail on a closed
// page. The session goes on from a blank tab: what the page showed is gone,
// and the next navigate loads whatever the grants and holds allow, as before.
async function keepPage() {
  if (!context || !page || !page.isClosed()) return;
  openingOwnTab = true;
  try {
    page = await context.newPage();
  } finally {
    openingOwnTab = false;
  }
  page.on("download", (download) => {
    download.cancel();
  });
}

function digest(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

// Asks the parent whether the operator lets this one request be sent. The
// parent sees what would be sent: method, whole URL, content type, size, the
// body's SHA-256 and its first characters as text. Anything but a clear yes
// inside the window is a refusal.
function operatorAllows(method, url, contentType, body, sha256, waitMs) {
  return new Promise((resolve) => {
    const id = (questions += 1);
    const askedAt = performance.now();
    const decide = (allow) => {
      // Decided once: by the parent's verdict, or here.
      if (!asked.delete(id)) return;
      clearTimeout(timer);
      // The running op waited on this question since it was asked, or since
      // the op began when the question was already open then.
      if (opStartedAt) opWaitedMs += performance.now() - Math.max(askedAt, opStartedAt);
      if (withdrawQuestion === refuse) withdrawQuestion = null;
      resolve(allow === true);
    };
    // The answer is no without the parent having given one: the wait ran out,
    // or the request is gone (cancelled, its page closed, or a navigate left
    // it). The parent is still asking its operator, so it is told: otherwise
    // it would show a request nobody can approve, and refuse the next
    // question as a second one, until its own wait ran out.
    const refuse = () => {
      if (reportWithdrawn && asked.has(id)) send({ withdrawn: id });
      decide(false);
    };
    const timer = setTimeout(refuse, waitMs);
    asked.set(id, decide);
    withdrawQuestion = refuse;
    send({ approve: id, wait_ms: waitMs, method, url, content_type: contentType.slice(0, 200), size: body.length, sha256, preview: body.toString("utf8").slice(0, MAX_PREVIEW_CHARS) });
  });
}

// How long a question asked now may wait, in whole milliseconds: the session's
// wait, less what the running op has already spent waiting.
function questionWindow() {
  return Math.max(0, Math.trunc(takeoverWaitMs - (opStartedAt ? opWaitedMs : 0)));
}

// Lists a questioned request as the operator's step, once.
function listOnce(entry) {
  if (entry.listed) return;
  entry.listed = true;
  if (held.length < 20) held.push(`${entry.method} ${entry.url.slice(0, 200)}`);
}

// The question, open or recently decided, about a request with this method
// and URL. One request waits at a time, so method and URL name it.
function questionFor(method, url) {
  const now = Date.now();
  return questioned.find((entry) => entry.method === method && entry.url === url && (!entry.until || entry.until > now));
}

// A request held as the operator's step, with takeover on: keep it paused,
// ask, then release this same request or hold it as before. Release only
// lets it leave the page route; the network gate still checks it against the
// pass recorded here, and against the block list as it is then.
// A request Chrome cancels or fails first, or whose page closes (the page
// left, or a click's new tab was adopted), is never sent by this route: an
// open question about it ends as a no, as an expired wait does, and a
// verdict that comes later finds no question. It is listed once.
async function decideHeld(route, request, method, url, waitMs) {
  const now = Date.now();
  for (let at = questioned.length - 1; at >= 0; at -= 1) {
    if (questioned[at].until && questioned[at].until <= now) questioned.splice(at, 1);
  }
  const entry = { method, url, listed: false, until: 0, withdraw: null };
  questioned.push(entry);
  let gone = false;
  const lost = () => {
    gone = true;
    entry.withdraw?.();
  };
  const failed = (other) => {
    if (other === request) lost();
  };
  let owner = null;
  try {
    owner = request.frame().page();
  } catch {}
  context.on("requestfailed", failed);
  owner?.on("close", lost);
  try {
    const pageLoad = request.isNavigationRequest();
    const body = request.postDataBuffer() ?? Buffer.alloc(0);
    const sha256 = digest(body);
    const contentType = (await request.headerValue("content-type").catch(() => null)) ?? "";
    let allow = false;
    if (!gone && !entry.listed) {
      const answer = operatorAllows(method, url, contentType, body, sha256, waitMs);
      entry.withdraw = withdrawQuestion;
      allow = (await answer) && !gone && !entry.listed;
    }
    entry.withdraw = null;
    if (allow) {
      const pass = { method, url, sha256, until: Date.now() + ALLOWANCE_MS };
      allowances.push(pass);
      // The action loaded a new page: what that page requests is its own doing.
      if (topLevelLoad(request)) leavePage();
      const changed = rewriteApproved ? { postData: Buffer.concat([body, Buffer.from("&changed=1")]) } : undefined;
      await route.continue(changed).catch(() => {});
      // Still this action's request until it is answered or fails, so the
      // observation that follows shows the page with the answer.
      await bounded(request.response(), "approved request").catch(() => {});
      // The network gate uses the pass up when the body matches. One still
      // here was not used: either the request never reached the gate, or the
      // gate found no pass for the body Chrome sent and listed it already
      // through this question's record. listOnce lists it once either way.
      const unused = allowances.indexOf(pass);
      if (unused < 0) {
        questioned.splice(questioned.indexOf(entry), 1);
        return;
      }
      allowances.splice(unused, 1);
    } else if (!gone) {
      await (pageLoad ? route.fulfill({ status: 204, body: "" }) : route.abort("blockedbyclient")).catch(() => {});
    }
    listOnce(entry);
  } finally {
    context.off("requestfailed", failed);
    owner?.off("close", lost);
    entry.withdraw = null;
    entry.until = Date.now() + ALLOWANCE_MS;
  }
}

// The body Chrome is about to send, as bytes, or null when Chrome did not
// hand it over (a large or streamed body): such a request has no pass.
function gateBody(request) {
  if (Array.isArray(request.postDataEntries)) {
    // An entry without bytes (a file part, say) is data nobody was shown.
    if (request.postDataEntries.some((entry) => typeof entry.bytes !== "string")) return null;
    return Buffer.concat(request.postDataEntries.map((entry) => Buffer.from(entry.bytes, "base64")));
  }
  if (typeof request.postData === "string") return Buffer.from(request.postData, "utf8");
  return request.hasPostData ? null : Buffer.alloc(0);
}

// Uses up the pass for this exact request, if the operator gave one.
function takeAllowance(request) {
  const body = gateBody(request);
  if (body === null) return false;
  const sha256 = digest(body);
  const now = Date.now();
  const at = allowances.findIndex((pass) => pass.until > now && pass.method === request.method && pass.url === request.url && pass.sha256 === sha256);
  if (at < 0) return false;
  allowances.splice(at, 1);
  return true;
}

// Records a request that could write to a server, for the operator. It was
// not sent. A page load always counts as the operator's step; any other
// request does only while the model's own action is running.
function hold(method, url, pageLoad) {
  const rows = pageLoad || acting ? held : heldBackground;
  if (rows.length < 20) rows.push(`${method} ${url.slice(0, 200)}`);
}

// A new top-level page is loading: neither the running action nor the one
// that just returned stands behind what that page sends.
function leavePage() {
  acting = false;
  lateSubmitUntil = 0;
}

// True for a POST page load of the page the model last acted on, inside the
// window after that action. Only the first one: asked about or not, it closes
// the window. A frame's or a new tab's load is not this page's.
function lateSubmit(request) {
  if (performance.now() >= lateSubmitUntil) return false;
  try {
    if (!request.isNavigationRequest() || request.frame() !== page.mainFrame()) return false;
  } catch {
    return false;
  }
  lateSubmitUntil = 0;
  return true;
}

// True for a request that loads a new top-level page. A new tab's first
// request has no frame yet, which Playwright reports by throwing.
function topLevelLoad(request) {
  if (!request.isNavigationRequest()) return false;
  try {
    return request.frame().parentFrame() === null;
  } catch {
    return true;
  }
}

// Every document a task loads carries this policy beside the site's own, and
// policies only narrow, so no page can start a dedicated or shared worker.
// Workers are where the per-page guards stop: context.route never sees a
// SharedWorker's requests, and routeWebSocket is a page script that a worker's
// WebSocket never meets. Blob, data and about:blank documents inherit it.
const WORKER_POLICY = { name: "Content-Security-Policy", value: "worker-src 'none'" };
// The largest document body the network gate reads before it re-sends it with
// WORKER_POLICY. Chrome hands the body over as a stream, read in chunks, so a
// bigger document is refused at the cap rather than after it is held whole.
const MAX_DOCUMENT_BYTES = 16 * 1024 * 1024;
const DOCUMENT_CHUNK_BYTES = 1024 * 1024;
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
    const declared = (event.responseHeaders || []).find(({ name }) => name.toLowerCase() === "content-length");
    const body = Number(declared?.value) > MAX_DOCUMENT_BYTES ? null : await boundedBody(cdp, requestId);
    if (body === null) {
      oversized += 1;
      return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
    }
    const headers = (event.responseHeaders || []).filter(({ name }) => !STALE_BODY_HEADERS.has(name.toLowerCase()));
    return cdp.send("Fetch.fulfillRequest", {
      requestId,
      responseCode: event.responseStatusCode,
      responsePhrase: event.responseStatusText || undefined,
      responseHeaders: [...headers, WORKER_POLICY],
      body: body.toString("base64"),
    });
  }
  const local = /^(data|blob|about):/.test(request.url);
  // The proxy refuses an ungranted origin too; the page route names it.
  if (!local && !allowed(request.url)) {
    return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    if (takeAllowance(request)) {
      // Approved a moment ago; the block list is asked again now.
      if (!local && !(await permitted(request.url))) {
        blocked += 1;
        if (event.resourceType === "Document") blockedLoads += 1;
        return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
      }
      if (approved.length < 20) approved.push(`${request.method} ${request.url.slice(0, 200)}`);
      return cdp.send("Fetch.continueRequest", { requestId });
    }
    // A questioned request that left the page route undecided: its page
    // closed. Its question ends now, and it is listed once, as asked.
    const question = questionFor(request.method, request.url);
    if (question) {
      question.withdraw?.();
      listOnce(question);
    } else hold(request.method, request.url, event.resourceType === "Document");
    // A held page load answers 204 so the page stays, as the page route does.
    if (event.resourceType === "Document") {
      return cdp.send("Fetch.fulfillRequest", { requestId, responseCode: 204, body: "" });
    }
    return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
  }
  // Last, and for every hop: a redirect's target pauses here as a new request,
  // so a block rule on a path holds for links, subresources and redirects alike.
  if (!local && !(await permitted(request.url))) {
    blocked += 1;
    if (event.resourceType === "Document") blockedLoads += 1;
    return cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
  }
  return cdp.send("Fetch.continueRequest", { requestId });
}

// A paused document's body, read through IO.read in DOCUMENT_CHUNK_BYTES
// pieces; null once it passes MAX_DOCUMENT_BYTES. After the stream is taken
// the request can only be fulfilled or failed, never continued as is.
async function boundedBody(cdp, requestId) {
  const { stream } = await cdp.send("Fetch.takeResponseBodyAsStream", { requestId });
  const parts = [];
  let total = 0;
  try {
    for (;;) {
      const { data, base64Encoded, eof } = await cdp.send("IO.read", { handle: stream, size: DOCUMENT_CHUNK_BYTES });
      const part = Buffer.from(data, base64Encoded ? "base64" : "utf8");
      total += part.length;
      if (total > MAX_DOCUMENT_BYTES) return null;
      parts.push(part);
      if (eof) return Buffer.concat(parts);
    }
  } finally {
    await cdp.send("IO.close", { handle: stream }).catch(() => {});
  }
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
  rewriteApproved = message.rewrite_approved_body === true;
  takeoverWaitMs = Math.max(0, Math.min(MAX_TAKEOVER_WAIT_MS, Math.trunc(Number(message.takeover_wait_ms) || 0)));
  reportWithdrawn = message.report_withdrawn === true;
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
      // The model's own action, with takeover on: pause it and ask. So is the
      // one form its click or Enter submits a moment after the op returned.
      // Any other form a page submits by itself is not asked about; it is
      // held below. The whole URL must fit the question, only one request
      // waits at a time, and the op must have wait left to spend on it.
      const late = lateSubmit(request);
      const waitMs = questionWindow();
      const worthAsking = waitMs >= Math.min(MIN_QUESTION_MS, takeoverWaitMs);
      if (takeoverWaitMs && (acting || late) && !pendingApproval && worthAsking && url.length <= MAX_URL) {
        const waiting = decideHeld(route, request, method, url, waitMs).finally(() => {
          if (pendingApproval === waiting) pendingApproval = null;
        });
        pendingApproval = waiting;
        askedThisOp = true;
        if (topLevelLoad(request)) loadStopped?.();
        return;
      }
      hold(method, url, request.isNavigationRequest());
      if (topLevelLoad(request)) loadStopped?.();
      // A held page load answers 204 so the page stays; a fetch/XHR fails.
      if (request.isNavigationRequest()) route.fulfill({ status: 204, body: "" });
      else route.abort("blockedbyclient");
      return;
    }
    // The action loaded a new page: what that page requests is its own doing.
    if (topLevelLoad(request)) leavePage();
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
  // Playwright dismisses a dialog nobody listens for. Listening must keep
  // that: an accepted confirm could delete something.
  context.on("dialog", (dialog) => {
    if (dialogs.length < MAX_DIALOGS) dialogs.push(`${dialog.type()}: ${dialog.message().slice(0, MAX_DIALOG_CHARS)}`);
    dialog.dismiss().catch(() => {});
  });
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
// After a held page load Playwright still counts that load as pending, so its
// domcontentloaded never comes; `loadHeld` reads the document's own state.
async function settle(loadHeld = false) {
  for (let pass = 0; pass < 2; pass += 1) {
    const parsed = loadHeld
      ? page.waitForFunction(() => document.readyState !== "loading", null, { timeout: 5000 })
      : page.waitForLoadState("domcontentloaded", { timeout: 5000 });
    await parsed.catch(() => {});
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

// Runs one Playwright action. An action that loads a page waits for that
// load, and gives up after its own few seconds. If the load is a request
// paused for the operator, that is not a failure: the observation that
// follows waits for the verdict instead.
async function acted(action) {
  try {
    await action;
  } catch (error) {
    if (!askedThisOp) throw error;
  }
}

async function locatorFor(target) {
  if (typeof target?.ref !== "string") throw new Error("stale or unknown target");
  const locator = page.getByRef(target.ref);
  if ((await locator.count().catch(() => 0)) !== 1) throw new Error("stale or unknown target");
  return locator;
}

// Watches, from now until the returned stop(), for a main-frame page load
// (in this tab or one it opens) redirected to an origin the task was not
// granted. The gate and the proxy refuse that hop whatever this sees; it only
// names the refusal, by origin alone, the same way for navigate and for the
// actions that load a page: `click`, `follow_link` and `press_enter`. A redirect
// a script starts during another action stays in the Page request details.
function watchRedirect() {
  let outside;
  // A throw here would end the sidecar, so nothing in it may.
  const seen = (response) => {
    try {
      if (outside || !response.request().isNavigationRequest()) return;
      if (![301, 302, 303, 307, 308].includes(response.status())) return;
      // A new tab's first load is issued before its frame exists, and
      // Playwright throws on frame() then; that load is the tab's main frame.
      let frame = null;
      try {
        frame = response.frame();
      } catch {}
      if (frame && frame.parentFrame() !== null) return;
      const location = response.headers().location;
      if (!location) return;
      const target = new URL(location, response.url()).href;
      if (!allowed(target)) outside = originOf(target);
    } catch {}
  };
  context.on("response", seen);
  return {
    origin: () => outside,
    stop: () => {
      context.off("response", seen);
      return outside;
    },
  };
}

function redirectRefused(origin) {
  return Object.assign(new Error("Page redirect requires a separate browser grant"), { redirectOrigin: origin });
}

// Chrome's page for a document the network gate failed. An iframe's error
// stays inside the granted page, so only a top-level page counts.
function blockedErrorPage() {
  return context.pages().some((open) => open.url().startsWith("chrome-error:"));
}

// A main-frame document navigation the block list refused. frame() throws
// for a new tab's first load, which is that tab's main frame. An iframe has
// a parent, so it does not count: blockedLoads moves for it too.
function watchBlockedNavigation() {
  let topLevel = false;
  const blockedByClient = (request) => (request.failure()?.errorText || "").includes("ERR_BLOCKED_BY_CLIENT");
  const seen = (request) => {
    try {
      if (topLevel || !request.isNavigationRequest() || !blockedByClient(request)) return;
      let frame = null;
      try {
        frame = request.frame();
      } catch {
        topLevel = true;
        return;
      }
      if (frame.parentFrame() === null) topLevel = true;
    } catch {}
  };
  context.on("requestfailed", seen);
  return {
    seen: () => topLevel,
    stop: () => context.off("requestfailed", seen),
  };
}

// The refusal navigate reports. `failed` is the evidence it was a main-frame
// load: goto rejected, or a click, link or Enter left that frame on the error
// page. The counter alone also moves for a blocked iframe.
function blockedLoadError(before, failed) {
  if (!failed || blockedLoads === before) return null;
  // Not the URL: the reason is the operator's rule, and the model has the URL.
  return new Error("the operator's web block list covers that page");
}

// A new tab can still be about:blank while its blocked navigation commits.
async function waitForBlockedTab() {
  const pending = context.pages().filter((open) => {
    const url = open.url();
    return url === "about:blank" || url === "";
  });
  if (!pending.length) return;
  await Promise.all(
    pending.map((open) => open.waitForLoadState("domcontentloaded", { timeout: 1000 }).catch(() => {})),
  );
}

// An action that can load a page. A refused redirect fails it with the
// redirect's origin, as navigate does. A block-listed main-frame load fails
// it with the same refusal. Either way it does not read as done.
async function loadingAction(run) {
  const before = blockedLoads;
  const redirect = watchRedirect();
  const blockedNav = watchBlockedNavigation();
  let failure;
  try {
    try {
      await run();
    } catch (error) {
      failure = error;
    }
    const outside = redirect.origin();
    if (outside) throw redirectRefused(outside);
    const actionBlocked = failure && String(failure.message || failure).includes("ERR_BLOCKED_BY_CLIENT");
    let failed = blockedNav.seen() || blockedErrorPage() || actionBlocked;
    if (!failed && blockedLoads !== before) {
      await waitForBlockedTab();
      failed = blockedNav.seen() || blockedErrorPage();
    }
    const blocked = blockedLoadError(before, failed);
    if (blocked) throw blocked;
    if (failure) throw failure;
    return observe();
  } finally {
    redirect.stop();
    blockedNav.stop();
  }
}

async function handle(message) {
  if (opening.size) await Promise.allSettled([...opening]);
  await keepPage();
  switch (message.op) {
    case "start":
      return start(message);
    case "navigate": {
      // A question still open here is about the form a click submitted late
      // on the page being left: the model has moved on, so the answer is no
      // and that form is held, rather than this navigate waiting on the operator.
      withdrawQuestion?.();
      while (pendingApproval) await pendingApproval;
      const before = blockedLoads;
      const large = oversized;
      // A failed goto names the starting URL even when a redirect was refused.
      const redirect = watchRedirect();
      // The page itself loading another page (a form it submits) and that load
      // held: what is shown now is the observation, not a failed navigate.
      // Only once this navigate's page committed: the old page's own load is not it.
      let committed = false;
      const commit = (frame) => {
        if (frame === page.mainFrame()) committed = true;
      };
      page.on("framenavigated", commit);
      const stopped = new Promise((resolve) => {
        loadStopped = () => {
          if (committed) resolve();
        };
      });
      let stoppedLoad = false;
      const loaded = page.goto(message.url, { waitUntil: "domcontentloaded", timeout: 10000 }).catch(async (error) => {
        if (stoppedLoad) return;
        const outside = redirect.origin();
        const deniedRedirect = outside && String(error.message).includes("ERR_BLOCKED_BY_CLIENT");
        const blocked = blockedLoadError(before, true);
        if (!deniedRedirect && !blocked && oversized === large) throw error;
        // Wait for Chrome's error page before the model tries another navigation.
        await page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame(), timeout: 1000 }).catch(() => {});
        if (deniedRedirect) throw redirectRefused(outside);
        if (oversized !== large) {
          throw new Error(`that page is larger than the ${MAX_DOCUMENT_BYTES / 1048576} MiB a browser task reads`);
        }
        if (blocked) throw blocked;
        throw error;
      });
      await Promise.race([loaded, stopped.then(() => { stoppedLoad = true; })]).finally(() => {
        loadStopped = null;
        redirect.stop();
        page.off("framenavigated", commit);
      });
      // A page that renders from a fetch is still a shell at domcontentloaded.
      await settle(stoppedLoad);
      return observe();
    }
    case "observe":
      return observe(message);
    case "click":
    case "follow_link": {
      const locator = await locatorFor(message.target);
      const tab = (await opensTab(locator)) ? page.waitForEvent("popup", { timeout: 5000 }).catch(() => null) : null;
      return loadingAction(async () => {
        await acted(locator.click({ timeout: 5000 }));
        await tab;
        await settle();
      });
    }
    case "type": {
      const locator = await locatorFor(message.target);
      await locator.fill(message.text, { timeout: 5000 });
      return observe();
    }
    case "press_enter": {
      const locator = await locatorFor(message.target);
      return loadingAction(async () => {
        await acted(locator.press("Enter", { timeout: 5000 }));
        await settle();
      });
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
      await acted(locator.selectOption({ index }, { timeout: 5000 }));
      await settle();
      return observe();
    }
    case "select_tab": {
      const locator = page.getByRole("tab", { name: message.tab, exact: true });
      if ((await bounded(locator.count(), "tab lookup")) !== 1) throw new Error("unknown tab");
      await acted(locator.click({ timeout: 5000 }));
      return observe();
    }
    case "scroll": {
      const dy = Math.max(-MAX_SCROLL_PX, Math.min(MAX_SCROLL_PX, Number(message.dy) || 0));
      await bounded(page.mouse.wheel(0, dy), "scroll");
      // The scroll itself and what it loads both land after the wheel event.
      await page.waitForTimeout(300);
      await settle();
      return observe();
    }
    case "wait":
      await page.waitForTimeout(Math.max(0, Math.min(MAX_WAIT_MS, Number(message.timeout_ms) || 0)));
      return observe();
    default:
      throw new Error("malformed browser action");
  }
}

async function answer(message) {
  const op = message?.op;
  acting = ACTS.has(op);
  // A new action or a navigate ends the window the last action left open.
  if (acting || op === "navigate") lateSubmitUntil = 0;
  askedThisOp = false;
  opStartedAt = performance.now();
  opWaitedMs = 0;
  let done = false;
  try {
    const result = await handle(message);
    done = true;
    send({ id: message.id, ok: true, result });
  } catch (error) {
    // The action failed after it had already caused these: a request held as
    // the operator's step ends the task, so it is reported with this reply
    // and not with whichever observation comes next, if one does. A question
    // the action raised is answered or timed out first (the op's questions
    // wait takeover_wait_ms in all, which the parent's call timeout allows
    // for), so its request is listed here as held or approved. One wait, not
    // a loop: a later question is the next reply's.
    if (pendingApproval) await pendingApproval;
    send({
      id: message?.id,
      ok: false,
      error: String(error.message || error).slice(0, 300),
      redirect_origin: error.redirectOrigin,
      held: held.splice(0),
      held_background: heldBackground.splice(0),
      approved: approved.splice(0),
    });
  } finally {
    // `acting` still set: the action loaded no new page. Nothing asked: its
    // request has not already been put to the operator.
    if (done && acting && !askedThisOp && SUBMITS.has(op)) lateSubmitUntil = performance.now() + LATE_SUBMIT_MS;
    acting = false;
    opStartedAt = 0;
  }
}

// Ops run one at a time, in order. A verdict is taken as its line arrives: the
// op that is running may be waiting on the very request it answers.
let ops = Promise.resolve();
const lines = createInterface({ input: process.stdin });
lines.on("line", (line) => {
  if (!line.trim()) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    send({ ok: false, error: String(error.message || error).slice(0, 300) });
    return;
  }
  if (message?.verdict !== undefined) asked.get(message.verdict)?.(message.allow);
  else ops = ops.then(() => answer(message));
});
await new Promise((resolve) => lines.once("close", resolve));
await ops;
// stdin closed: the parent is gone or done. Close Chrome rather than orphan it.
await context?.close().catch(() => {});
if (profileDir) fs.rmSync(profileDir, { recursive: true, force: true });
process.exit(0);
