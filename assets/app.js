// Wiretoss 前端。
//
// 刻意不引入框架和构建步骤：整个界面就是一个文件列表加一个传输面板，
// 上 Vite/React 只会让"改一行文案"变成"先 npm install"。
// 所有用户可见的文本都走 textContent，绝不拼 innerHTML —— 文件名是
// 完全由对端控制的输入，拼字符串就等于开了一个 XSS 口子。

const CHUNK_SIZE = 8 * 1024 * 1024;
const FILE_CONCURRENCY = 3;
const CHUNK_CONCURRENCY = 2;
const CSRF_HEADER = { 'X-Wiretoss': '1' };

const $ = (id) => document.getElementById(id);

const ICON_DIR = 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z';
const ICON_FILE = 'M13 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9zM13 3v6h6';

// ─────────────────────────────── 状态 ───────────────────────────────

const state = {
  cwd: '',
  entries: [],
  selected: new Set(),
  sort: { key: 'name', dir: 1 },
  loggedIn: false,
};

/** 进行中的上传任务，key 是自增 id。 */
const transfers = new Map();
let transferSeq = 0;

// ─────────────────────────────── 工具 ───────────────────────────────

const joinPath = (a, b) => (a && b ? `${a}/${b}` : a || b || '');

function formatSize(n) {
  if (!n) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${i > 0 && n < 10 ? n.toFixed(1) : Math.round(n)} ${units[i]}`;
}

function formatTime(secs) {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  const pad = (x) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function formatDuration(secs) {
  if (!isFinite(secs) || secs < 0) return '';
  if (secs < 60) return `${Math.ceil(secs)} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分 ${Math.round(secs % 60)} 秒`;
  return `${Math.floor(secs / 3600)} 小时 ${Math.round((secs % 3600) / 60)} 分`;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function svgIcon(path) {
  const el = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  el.setAttribute('viewBox', '0 0 24 24');
  el.setAttribute('class', 'icon');
  el.innerHTML = `<path d="${path}"/>`;
  return el;
}

function toast(message, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = message;
  $('toasts').append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 6000 : 3200);
}

// ─────────────────────────────── API ───────────────────────────────

class AuthError extends Error {}

async function api(path, { method = 'GET', body } = {}) {
  const init = { method, headers: { ...CSRF_HEADER } };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const res = await fetch(path, init);
  if (res.status === 401) throw new AuthError('未登录');

  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* 非 JSON 响应 */ }
  if (!res.ok) throw new Error(data?.error || `请求失败（HTTP ${res.status}）`);
  return data;
}

// ─────────────────────────────── 登录 ───────────────────────────────

async function boot() {
  try {
    const me = await api('/api/me');
    showApp(me);
  } catch (e) {
    if (e instanceof AuthError) showLogin();
    else { showLogin(); toast(e.message, 'error'); }
  }
}

function showLogin() {
  state.loggedIn = false;
  $('app-view').hidden = true;
  $('login-view').hidden = false;
  $('login-pass').value = '';
  $('login-user').focus();
}

function showApp(me) {
  state.loggedIn = true;
  $('login-view').hidden = true;
  $('app-view').hidden = false;
  $('user-label').textContent = `${me.username} · ${me.rootName || me.rootPath || ''}`;
  connectEvents();
  navigate('');
}

$('login-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const btn = $('login-submit');
  const err = $('login-error');
  err.hidden = true;
  btn.disabled = true;
  btn.textContent = '登录中…';
  try {
    await api('/api/login', {
      method: 'POST',
      body: { username: $('login-user').value, password: $('login-pass').value },
    });
    await boot();
  } catch (e) {
    err.textContent = e.message;
    err.hidden = false;
    $('login-pass').select();
  } finally {
    btn.disabled = false;
    btn.textContent = '登录';
  }
});

$('logout-btn').addEventListener('click', async () => {
  try { await api('/api/logout', { method: 'POST' }); } catch { /* 忽略 */ }
  location.reload();
});

// ─────────────────────────────── 目录浏览 ───────────────────────────────

async function navigate(path) {
  try {
    const data = await api(`/api/list?path=${encodeURIComponent(path)}`);
    state.cwd = data.path;
    state.entries = data.entries;
    state.selected.clear();
    renderCrumbs();
    renderList();
  } catch (e) {
    if (e instanceof AuthError) return showLogin();
    toast(e.message, 'error');
  }
}

async function refresh() {
  if (!state.loggedIn) return;
  try {
    const data = await api(`/api/list?path=${encodeURIComponent(state.cwd)}`);
    state.entries = data.entries;
    // 目录内容变了，之前选中的条目可能已经不在了，清掉免得误操作。
    for (const name of [...state.selected]) {
      if (!state.entries.some((e) => e.name === name)) state.selected.delete(name);
    }
    renderList();
  } catch (e) {
    if (e instanceof AuthError) showLogin();
  }
}

function renderCrumbs() {
  const nav = $('crumbs');
  nav.replaceChildren();

  const add = (label, path, current) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'crumb';
    b.textContent = label;
    if (current) b.setAttribute('aria-current', 'page');
    b.addEventListener('click', () => navigate(path));
    nav.append(b);
  };

  add('共享目录', '', state.cwd === '');
  let acc = '';
  const parts = state.cwd ? state.cwd.split('/') : [];
  parts.forEach((part, i) => {
    const sep = document.createElement('span');
    sep.className = 'crumb-sep';
    sep.textContent = '/';
    nav.append(sep);
    acc = joinPath(acc, part);
    add(part, acc, i === parts.length - 1);
  });
}

function sortedEntries() {
  const { key, dir } = state.sort;
  return [...state.entries].sort((a, b) => {
    // 目录永远排在文件前面，与排序键无关。
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    let r = 0;
    if (key === 'size') r = a.size - b.size;
    else if (key === 'mtime') r = a.mtime - b.mtime;
    // numeric 让 "文件2" 排在 "文件10" 前面，符合直觉。
    else r = a.display_name.localeCompare(b.display_name, 'zh-Hans-CN', { numeric: true });
    return r * dir;
  });
}

function renderList() {
  const tbody = $('file-tbody');
  tbody.replaceChildren();
  const entries = sortedEntries();
  $('empty-hint').hidden = entries.length > 0;

  for (const entry of entries) {
    tbody.append(buildRow(entry));
  }
  updateSelectionUI();
}

function buildRow(entry) {
  const tr = document.createElement('tr');
  if (state.selected.has(entry.name)) tr.classList.add('selected');

  // 勾选
  const tdCheck = document.createElement('td');
  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.checked = state.selected.has(entry.name);
  cb.setAttribute('aria-label', `选择 ${entry.display_name}`);
  cb.addEventListener('change', () => {
    if (cb.checked) state.selected.add(entry.name);
    else state.selected.delete(entry.name);
    tr.classList.toggle('selected', cb.checked);
    updateSelectionUI();
  });
  tdCheck.append(cb);
  tr.append(tdCheck);

  // 名称
  const tdName = document.createElement('td');
  const cell = document.createElement('div');
  cell.className = `name-cell${entry.is_dir ? ' is-dir' : ''}`;
  cell.append(svgIcon(entry.is_dir ? ICON_DIR : ICON_FILE));

  const nameBtn = document.createElement('button');
  nameBtn.type = 'button';
  nameBtn.className = `name-text${entry.is_dir && !entry.restricted ? '' : ' plain'}`;
  nameBtn.textContent = entry.display_name;
  nameBtn.title = entry.display_name;
  if (entry.is_dir && !entry.restricted) {
    nameBtn.addEventListener('click', () => navigate(joinPath(state.cwd, entry.name)));
  } else if (!entry.is_dir) {
    nameBtn.addEventListener('click', () => downloadOne(entry.name));
  }
  cell.append(nameBtn);

  if (entry.restricted) {
    const badge = document.createElement('span');
    badge.className = 'restricted-badge';
    badge.textContent = '指向共享目录外';
    cell.append(badge);
  }
  tdName.append(cell);
  tr.append(tdName);

  // 大小
  const tdSize = document.createElement('td');
  tdSize.className = 'muted';
  tdSize.textContent = entry.is_dir ? '—' : formatSize(entry.size);
  tr.append(tdSize);

  // 时间
  const tdTime = document.createElement('td');
  tdTime.className = 'muted';
  tdTime.textContent = formatTime(entry.mtime);
  tr.append(tdTime);

  // 操作
  const tdAct = document.createElement('td');
  const actions = document.createElement('div');
  actions.className = 'row-actions';
  if (entry.restricted) {
    // 受限条目什么都不能做，不给按钮比给了再报错更清楚。
  } else if (entry.is_dir) {
    actions.append(rowButton('打包下载', () => downloadZip([entry.name])));
  } else {
    actions.append(rowButton('下载', () => downloadOne(entry.name)));
  }
  actions.append(rowButton('删除', () => removePaths([entry.name])));
  tdAct.append(actions);
  tr.append(tdAct);

  return tr;
}

function rowButton(label, onClick) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = 'btn btn-ghost btn-sm';
  b.textContent = label;
  b.addEventListener('click', onClick);
  return b;
}

function updateSelectionUI() {
  const n = state.selected.size;
  $('download-btn').disabled = n === 0;
  $('delete-btn').disabled = n === 0;
  $('download-btn').textContent = n ? `下载所选（${n}）` : '下载所选';
  const all = state.entries.length > 0 && n === state.entries.length;
  $('check-all').checked = all;
  $('check-all').indeterminate = n > 0 && !all;
}

$('check-all').addEventListener('change', (ev) => {
  state.selected.clear();
  if (ev.target.checked) for (const e of state.entries) state.selected.add(e.name);
  renderList();
});

for (const th of document.querySelectorAll('th[data-sort]')) {
  th.addEventListener('click', () => {
    const key = th.dataset.sort;
    if (state.sort.key === key) state.sort.dir *= -1;
    else state.sort = { key, dir: key === 'name' ? 1 : -1 };
    for (const other of document.querySelectorAll('th[data-sort]')) other.removeAttribute('aria-sort');
    th.setAttribute('aria-sort', state.sort.dir === 1 ? 'ascending' : 'descending');
    renderList();
  });
}

// ─────────────────────────────── 下载 ───────────────────────────────

function downloadOne(name) {
  const path = joinPath(state.cwd, name);
  // 走浏览器原生下载：它能落盘流式写入、自带进度和断点续传，
  // 用 fetch 拿字节再存反而要整个文件先进内存。
  location.href = `/api/download?path=${encodeURIComponent(path)}`;
}

async function downloadZip(names) {
  try {
    const ticket = await api('/api/zip', {
      method: 'POST',
      body: { paths: names.map((n) => joinPath(state.cwd, n)) },
    });
    // 券是一次性的，导航过去即可触发流式下载。
    location.href = `/api/zip/${ticket.ticket}`;
  } catch (e) {
    if (e instanceof AuthError) return showLogin();
    toast(e.message, 'error');
  }
}

$('download-btn').addEventListener('click', () => {
  const names = [...state.selected];
  if (names.length === 1) {
    const entry = state.entries.find((e) => e.name === names[0]);
    if (entry && !entry.is_dir) return downloadOne(names[0]);
  }
  downloadZip(names);
});

// ─────────────────────────────── 删除 / 新建 ───────────────────────────────

async function removePaths(names) {
  const label = names.length === 1 ? `「${names[0]}」` : `选中的 ${names.length} 项`;
  if (!confirm(`确定要删除 ${label} 吗？此操作不可撤销。`)) return;
  try {
    await api('/api/delete', {
      method: 'POST',
      body: { paths: names.map((n) => joinPath(state.cwd, n)) },
    });
    state.selected.clear();
    toast('已删除', 'ok');
    refresh();
  } catch (e) {
    if (e instanceof AuthError) return showLogin();
    toast(e.message, 'error');
  }
}

$('delete-btn').addEventListener('click', () => removePaths([...state.selected]));

$('mkdir-btn').addEventListener('click', async () => {
  const name = prompt('新建文件夹名称');
  if (!name) return;
  try {
    await api('/api/mkdir', { method: 'POST', body: { dir: state.cwd, name } });
    refresh();
  } catch (e) {
    if (e instanceof AuthError) return showLogin();
    toast(e.message, 'error');
  }
});

// ─────────────────────────────── 上传 ───────────────────────────────

$('upload-files').addEventListener('click', () => $('file-input').click());
$('upload-folder').addEventListener('click', () => $('dir-input').click());
$('file-input').addEventListener('change', (e) => { enqueueFiles(e.target.files); e.target.value = ''; });
$('dir-input').addEventListener('change', (e) => { enqueueFiles(e.target.files); e.target.value = ''; });

/**
 * 把一批 File 排进上传队列。文件夹上传时浏览器会给每个 File 带上
 * `webkitRelativePath`（形如 `照片/2024/a.jpg`），据此先在服务端补建目录。
 */
async function enqueueFiles(fileList) {
  const files = [...fileList];
  if (!files.length) return;

  // 1. 收集所有需要的中间目录
  const needed = new Set();
  for (const f of files) {
    const segs = (f.webkitRelativePath || f.name).split('/');
    segs.pop();
    let acc = '';
    for (const s of segs) { acc = joinPath(acc, s); needed.add(acc); }
  }

  // 2. 按层级从浅到深创建。必须逐级创建，因为服务端每次只接受一个文件名段。
  const ordered = [...needed].sort((a, b) => a.split('/').length - b.split('/').length);
  for (const rel of ordered) {
    const segs = rel.split('/');
    const name = segs.pop();
    try {
      await api('/api/mkdir', { method: 'POST', body: { dir: joinPath(state.cwd, segs.join('/')), name } });
    } catch { /* 已存在，忽略 */ }
  }

  // 3. 排进上传队列
  for (const f of files) {
    const segs = (f.webkitRelativePath || f.name).split('/');
    const name = segs.pop();
    const dir = joinPath(state.cwd, segs.join('/'));
    queueUpload(f, dir, name);
  }
}

const uploadQueue = [];
let activeUploads = 0;

function queueUpload(file, dir, name) {
  uploadQueue.push({ file, dir, name });
  pumpQueue();
}

function pumpQueue() {
  while (activeUploads < FILE_CONCURRENCY && uploadQueue.length) {
    const job = uploadQueue.shift();
    activeUploads++;
    runUpload(job).finally(() => { activeUploads--; pumpQueue(); });
  }
}

async function runUpload({ file, dir, name }) {
  const t = createTransfer(name, file.size);
  try {
    const init = await api('/api/upload/init', {
      method: 'POST',
      body: {
        name,
        size: file.size,
        last_modified: Math.floor(file.lastModified / 1000),
        dir,
      },
    });
    t.uploadId = init.upload_id;

    const done = new Set(init.received_chunks);
    if (init.resumed) {
      toast(`「${name}」从上次中断处继续（已完成 ${done.size}/${init.total_chunks} 片）`);
    }
    updateTransfer(t, done.size * CHUNK_SIZE, file.size);

    const pending = [];
    for (let i = 0; i < init.total_chunks; i++) if (!done.has(i)) pending.push(i);

    // 分片并发：局域网单连接就接近跑满带宽，并行主要是为了掩盖丢包重传的停顿。
    let cursor = 0;
    const workers = Array.from(
      { length: Math.min(CHUNK_CONCURRENCY, pending.length) },
      async () => {
        while (cursor < pending.length) {
          const idx = pending[cursor++];
          const start = idx * CHUNK_SIZE;
          const blob = file.slice(start, Math.min(start + CHUNK_SIZE, file.size));
          await putChunk(init.upload_id, idx, blob);
          done.add(idx);
          updateTransfer(t, Math.min(done.size * CHUNK_SIZE, file.size), file.size);
        }
      },
    );
    await Promise.all(workers);

    await api(`/api/upload/${init.upload_id}/complete`, { method: 'POST' });
    finishTransfer(t);
  } catch (e) {
    failTransfer(t, e instanceof AuthError ? '登录已过期' : e.message);
    if (e instanceof AuthError) showLogin();
  }
}

/** 单个分片的 PUT，带指数退避重试。 */
async function putChunk(uploadId, index, blob) {
  let lastError;
  for (let attempt = 0; attempt < 4; attempt++) {
    try {
      const res = await fetch(`/api/upload/chunk/${uploadId}/${index}`, {
        method: 'PUT',
        headers: { ...CSRF_HEADER },
        body: blob,
      });
      if (res.ok) return;
      if (res.status === 401) throw new AuthError('未登录');
      const data = await res.json().catch(() => null);
      lastError = new Error(data?.error || `分片上传失败（HTTP ${res.status}）`);
    } catch (e) {
      lastError = e;
    }
    // 最后一轮失败就别再等了，直接把错误抛出去。
    if (attempt < 3) await sleep(300 * 2 ** attempt);
  }
  throw lastError;
}

// ─────────────────────────────── 传输面板 ───────────────────────────────

function createTransfer(name, size) {
  const id = ++transferSeq;
  const li = document.createElement('li');
  li.className = 'transfer';

  const top = document.createElement('div');
  top.className = 'transfer-top';
  const nameEl = document.createElement('span');
  nameEl.className = 'transfer-name';
  nameEl.textContent = name;
  nameEl.title = name;
  const statEl = document.createElement('span');
  statEl.className = 'transfer-stat';
  statEl.textContent = '准备中…';
  top.append(nameEl, statEl);

  const bar = document.createElement('div');
  bar.className = 'bar';
  const fill = document.createElement('i');
  bar.append(fill);

  li.append(top, bar);
  $('transfer-list').append(li);
  $('transfers').hidden = false;

  const t = { id, li, statEl, fill, size, loaded: 0, startedAt: Date.now() };
  transfers.set(id, t);
  return t;
}

function updateTransfer(t, loaded, size) {
  t.loaded = loaded;
  const pct = size > 0 ? (loaded / size) * 100 : 100;
  t.fill.style.width = `${Math.min(100, pct)}%`;

  const elapsed = (Date.now() - t.startedAt) / 1000;
  const speed = elapsed > 0.4 ? loaded / elapsed : 0;
  const parts = [`${pct.toFixed(0)}%`, `${formatSize(loaded)} / ${formatSize(size)}`];
  if (speed > 0) {
    parts.push(`${formatSize(speed)}/s`);
    if (loaded < size) parts.push(`剩 ${formatDuration((size - loaded) / speed)}`);
  }
  t.statEl.textContent = parts.join(' · ');
}

function finishTransfer(t) {
  t.li.classList.add('done');
  t.fill.style.width = '100%';
  const elapsed = (Date.now() - t.startedAt) / 1000;
  const avg = elapsed > 0 ? t.size / elapsed : 0;
  t.statEl.textContent = `完成 · ${formatSize(t.size)}${avg > 0 ? ` · 平均 ${formatSize(avg)}/s` : ''}`;
  transfers.delete(t.id);
  schedulePanelCleanup();
  refresh();
}

function failTransfer(t, message) {
  t.li.classList.add('error');
  t.statEl.textContent = message;
  transfers.delete(t.id);
  schedulePanelCleanup();
  toast(message, 'error');
}

/** 全部传完后自动收起面板；还有任务就留着。 */
function schedulePanelCleanup() {
  setTimeout(() => {
    if (transfers.size === 0) $('transfers').hidden = true;
  }, 6000);
}

$('transfers-toggle').addEventListener('click', () => {
  const panel = $('transfers');
  panel.classList.toggle('collapsed');
  $('transfers-toggle').textContent = panel.classList.contains('collapsed') ? '展开' : '收起';
});

// ─────────────────────────────── 拖拽上传 ───────────────────────────────

let dragDepth = 0;

window.addEventListener('dragenter', (e) => {
  if (!state.loggedIn || !e.dataTransfer?.types.includes('Files')) return;
  e.preventDefault();
  dragDepth++;
  $('drop-overlay').hidden = false;
});

window.addEventListener('dragover', (e) => {
  if (!state.loggedIn || !e.dataTransfer?.types.includes('Files')) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = 'copy';
});

window.addEventListener('dragleave', () => {
  // dragleave 会在每个子元素上冒泡触发，用计数器才不会闪。
  if (dragDepth > 0) dragDepth--;
  if (dragDepth === 0) $('drop-overlay').hidden = true;
});

window.addEventListener('drop', async (e) => {
  if (!state.loggedIn) return;
  e.preventDefault();
  dragDepth = 0;
  $('drop-overlay').hidden = true;

  const items = [...(e.dataTransfer?.items || [])];
  const entries = items.map((i) => i.webkitGetAsEntry?.()).filter(Boolean);

  // 支持拖文件夹：递归读出目录树。浏览器没有批量 API，
  // 只能靠 FileSystemDirectoryReader 一层层读。
  if (entries.some((en) => en.isDirectory)) {
    const files = [];
    for (const en of entries) await walkEntry(en, '', files);
    enqueueFiles(files);
  } else {
    enqueueFiles(e.dataTransfer.files);
  }
});

function walkEntry(entry, prefix, out) {
  return new Promise((resolve) => {
    if (entry.isFile) {
      entry.file(
        (file) => {
          // 伪造 webkitRelativePath，让后续逻辑和 <input webkitdirectory> 走同一条路。
          const rel = joinPath(prefix, file.name);
          Object.defineProperty(file, 'webkitRelativePath', { value: rel });
          out.push(file);
          resolve();
        },
        () => resolve(),
      );
      return;
    }
    const reader = entry.createReader();
    const readBatch = () => {
      reader.readEntries(
        async (batch) => {
          if (!batch.length) return resolve();
          for (const child of batch) await walkEntry(child, joinPath(prefix, entry.name), out);
          readBatch(); // readEntries 一次最多返回 100 项，要反复读到空为止
        },
        () => resolve(),
      );
    };
    readBatch();
  });
}

// ─────────────────────────────── 实时事件 ───────────────────────────────

let eventSource = null;

function connectEvents() {
  if (eventSource) eventSource.close();
  eventSource = new EventSource('/api/events');

  eventSource.onmessage = (ev) => {
    let data;
    try { data = JSON.parse(ev.data); } catch { return; }
    handleEvent(data);
  };

  // 落后于广播缓冲区时前端可能漏掉了事件，直接全量刷新补回来。
  eventSource.addEventListener('lagged', () => refresh());
}

function handleEvent(data) {
  switch (data.kind) {
    case 'files_changed':
      if (data.dir === state.cwd) refresh();
      break;
    case 'upload_finished':
      // 可能是对端传过来的，提示一下——这正是这个面板存在的意义。
      toast(`已收到「${data.name}」`, 'ok');
      break;
    default:
      break;
  }
}

// ─────────────────────────────── 启动 ───────────────────────────────

boot();
