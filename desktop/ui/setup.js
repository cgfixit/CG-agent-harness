'use strict';
const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
const status = document.getElementById('status');
const details = document.getElementById('details');
const codeRow = document.getElementById('codeRow');
const codeText = document.getElementById('setupCode');
const copyButton = document.getElementById('copyCode');
// Rewritten only when the code changes, so a selection survives the 2 s poll.
function showCode(code) {
  code = code || '';
  if (codeText.textContent !== code) { codeText.textContent = code; copyButton.textContent = 'Copy setup code'; }
  codeRow.hidden = !code;
}
// Only on a click: never put the credential on the clipboard unasked.
async function copyCode() {
  const code = codeText.textContent;
  if (!code) return;
  try { await navigator.clipboard.writeText(code); copyButton.textContent = 'Copied'; return; } catch (_) { /* fall back below */ }
  try {
    const range = document.createRange();
    range.selectNodeContents(codeText);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    if (document.execCommand('copy')) { copyButton.textContent = 'Copied'; return; }
  } catch (_) { /* leave it for Cmd-C */ }
  copyButton.textContent = 'Select the code and press Cmd-C';
}
let busy = false;
let failure = '';
let actionVersion = 0;
async function refresh() {
  if (busy || failure) return;
  const version = actionVersion;
  try {
    const data = await invoke('desktop_status');
    if (version !== actionVersion) return;
    status.textContent = data.message;
    showCode(data.setup_code);
    document.getElementById('certificateRow').hidden = !data.certificate_sha256;
    document.getElementById('certificateFingerprint').textContent = data.certificate_sha256 || '';
    details.textContent = data.details;
  }
  catch (_) {
    if (version !== actionVersion) return;
    status.textContent = 'Setup connection unavailable. Quit and reopen the app.'; details.textContent = '';
  }
}
async function action(command, args) {
  actionVersion++;
  busy = true; failure = ''; details.textContent = '';
  for (const b of document.querySelectorAll('button')) b.disabled = true;
  try {
    status.textContent = 'Working…';
    const result = await invoke(command, args);
    if (command === 'export_browser_certificate') document.getElementById('certificateResult').textContent = result ? 'Public certificate saved to ' + result + '. Review its fingerprint and trust settings in your browser or Keychain Access.' : 'Export cancelled.';
  }
  catch (e) { failure = String(e); status.textContent = failure; }
  finally { busy = false; for (const b of document.querySelectorAll('button')) b.disabled = false; refresh(); }
}
document.getElementById('retry').addEventListener('click', () => action('retry_backend', { initializeKey: null }));
document.getElementById('prepare').addEventListener('click', () => action('prepare_cargo'));
copyButton.addEventListener('click', copyCode);
document.getElementById('models').addEventListener('click', () => action('check_models'));
document.getElementById('exportCertificate').addEventListener('click', () => action('export_browser_certificate'));
refresh(); setInterval(refresh, 2000);
