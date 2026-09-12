'use strict';
const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
const status = document.getElementById('status');
const details = document.getElementById('details');
let busy = false;
let failure = '';
async function refresh() {
  if (busy || failure) return;
  try { const data = await invoke('desktop_status'); status.textContent = data.message; details.textContent = data.details; }
  catch (_) { status.textContent = 'Setup connection unavailable. Quit and reopen the app.'; }
}
async function action(command, args) {
  busy = true; failure = '';
  for (const b of document.querySelectorAll('button')) b.disabled = true;
  try { status.textContent = 'Working…'; await invoke(command, args); }
  catch (e) { failure = String(e); status.textContent = failure; }
  finally { busy = false; for (const b of document.querySelectorAll('button')) b.disabled = false; refresh(); }
}
document.getElementById('retry').addEventListener('click', () => action('retry_backend', { initializeKey: null }));
document.getElementById('prepare').addEventListener('click', () => action('prepare_cargo'));
document.getElementById('models').addEventListener('click', () => action('check_models'));
refresh(); setInterval(refresh, 2000);
