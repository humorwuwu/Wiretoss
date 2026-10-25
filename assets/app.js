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
const ICON_DOWNLOAD = 'M12 4v12m0 0 4-4m-4 4-4-4M4 20h16';
const ICON_ZIP = 'M21 8l-9-5-9 5 9 5 9-5zM3 8v8l9 5 9-5V8M12 13v8';
const ICON_TRASH = 'M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3';

// 按扩展名分组的文件图标。视觉上给不同类别不同色调，
// 文件多时不用逐个读名字也能快速扫到目标。
const FILE_ICONS = {
  image: 'M4 5a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2zM8.5 10.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM21 15l-5-5L5 21',
  video: 'M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2zM10 9l5 3-5 3z',
  audio: 'M9 18V5l10-2v13M9 18a3 3 0 1 1-4-3 3 3 0 0 1 4 3zM19 16a3 3 0 1 1-4-3 3 3 0 0 1 4 3z',
  archive: ICON_ZIP,
  doc: 'M13 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9zM13 3v6h6M9 13h6M9 17h4',
  code: 'M8 9l-3 3 3 3M16 9l3 3-3 3M13 6l-2 12',
};

const FILE_EXT = {
  image: 'png jpg jpeg gif webp heic heif avif bmp tif tiff svg ico',
  video: 'mp4 mov mkv avi webm m4v wmv flv mpg mpeg',
  audio: 'mp3 wav flac aac m4a ogg opus aiff',
  archive: 'zip rar 7z tar gz bz2 xz zst dmg iso apk ipa',
  doc: 'pdf doc docx xls xlsx ppt pptx txt md rtf csv pages numbers key epub',
  code: 'rs js mjs ts tsx jsx html htm css scss json toml yaml yml xml py go java kt c h cpp hpp cs sh bash zsh sql gradle lock',
};

const KIND_BY_EXT = Object.fromEntries(
  Object.entries(FILE_EXT).flatMap(([kind, exts]) => exts.split(' ').map((ext) => [ext, kind])),
);

function fileKind(name) {
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return 'file';
  return KIND_BY_EXT[name.slice(dot + 1).toLowerCase()] || 'file';
}

// ─────────────────────────────── 状态 ───────────────────────────────

const state = {
  cwd: '',
  entries: [],
  selected: new Set(),
  sort: { key: 'name', dir: 1 },
  loggedIn: false,
  canWrite: true,
  allowGuest: false,
  canLogin: false,
  httpsEnabled: false,
  httpsPort: 0,
  localDirHandle: null,
  localDirName: '',
  localSaveBusy: false,
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

// 原生 confirm/prompt 在深色界面里非常突兀，而且手机上样式完全不可控。
// 这里用一个很小的 Promise 对话框替代，视觉和主界面保持一致。
function uiDialog({
  title = '提示',
  message = '',
  input = null,
  placeholder = '',
  confirmText = '确定',
  danger = false,
} = {}) {
  return new Promise((resolve) => {
    const root = $('dialog-root');
    const inputEl = $('dialog-input');
    const confirm = $('dialog-confirm');
    const cancel = $('dialog-cancel');
    const backdrop = root.querySelector('[data-dialog-cancel]');
    const app = $('app-view');
    const previousFocus = document.activeElement;

    // 对话框打开期间让后面的应用不可聚焦；inert 不支持的浏览器会忽略这一行。
    app.inert = true;

    $('dialog-title').textContent = title;
    $('dialog-message').textContent = message;
    $('dialog-message').hidden = !message;
    confirm.textContent = confirmText;
    confirm.classList.toggle('btn-danger', danger);
    confirm.classList.toggle('btn-primary', !danger);
    inputEl.hidden = input === null;
    inputEl.value = input ?? '';
    inputEl.placeholder = placeholder;
    root.hidden = false;

    const finish = (value) => {
      root.hidden = true;
      document.removeEventListener('keydown', onKey, true);
      confirm.onclick = null;
      cancel.onclick = null;
      backdrop.onclick = null;
      app.inert = false;
      if (previousFocus && typeof previousFocus.focus === 'function') {
        previousFocus.focus();
      }
      resolve(value);
    };

    // 挂在 document 上而不是对话框根节点上：对话框刚打开的那几毫秒里
    // 焦点可能还在后面的按钮上，只挂根节点会漏掉这一次 Escape。
    const onKey = (ev) => {
      if (ev.key === 'Escape') {
        ev.preventDefault();
        finish(null);
      } else if (ev.key === 'Enter' && document.activeElement === inputEl) {
        ev.preventDefault();
        confirm.click();
      }
    };
    document.addEventListener('keydown', onKey, true);

    confirm.onclick = () => {
      if (input !== null && !inputEl.value.trim()) {
        inputEl.focus();
        return;
      }
      finish(input !== null ? inputEl.value.trim() : true);
    };
    cancel.onclick = () => finish(null);
    backdrop.onclick = () => finish(null);

    setTimeout(() => {
      if (input !== null) {
        inputEl.focus();
        inputEl.select();
      } else {
        confirm.focus();
      }
    }, 0);
  });
}

const uiConfirm = (message, options = {}) => uiDialog({ message, ...options }).then((v) => v === true);
const uiPrompt = (title, options = {}) => uiDialog({ title, input: '', ...options });

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
    if (e instanceof AuthError) {
      // 服务端要求登录。之前的访客状态可能已经失效，不能再显示“返回只读”。
      state.allowGuest = false;
      showLogin();
    } else {
      showLogin();
      toast(e.message, 'error');
    }
  }
}

function showLogin() {
  state.loggedIn = false;
  document.title = 'Wiretoss';
  $('app-view').hidden = true;
  $('login-view').hidden = false;
  $('login-back').hidden = !(state.allowGuest && state.canLogin);
  $('login-pass').value = '';
  $('login-user').focus();
}

function showApp(me) {
  state.loggedIn = true;
  state.canWrite = me.canWrite !== false;
  state.allowGuest = me.allowGuest === true;
  state.canLogin = me.canLogin === true;
  document.body.classList.toggle('read-only', !state.canWrite);

  $('login-view').hidden = true;
  $('app-view').hidden = false;

  const root = me.rootName || me.rootPath || '';
  const label = state.canWrite ? me.username : '只读访客';
  $('user-label').textContent = label;
  $('user-label').title = me.rootPath || root;
  $('sidebar-user').textContent = label;
  $('sidebar-root').textContent = root || '—';
  $('sidebar-root').title = me.rootPath || root;
  $('login-btn').hidden = state.canWrite || !state.allowGuest || !state.canLogin;
  $('logout-btn').hidden = !state.canWrite;

  void initLocalSave(me);
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

$('login-btn').addEventListener('click', () => {
  showLogin();
});

$('login-back').addEventListener('click', () => {
  boot();
});

$('password-toggle').addEventListener('click', () => {
  const input = $('login-pass');
  const button = $('password-toggle');
  const show = input.type === 'password';
  input.type = show ? 'text' : 'password';
  button.classList.toggle('is-on', show);
  button.setAttribute('aria-pressed', String(show));
  button.setAttribute('aria-label', show ? '隐藏密码' : '显示密码');
  input.focus();
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
  document.title = state.cwd ? `${state.cwd.split('/').pop()} · Wiretoss` : 'Wiretoss';

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

  const dirs = entries.filter((e) => e.is_dir).length;
  const bytes = entries.reduce((sum, e) => sum + (e.is_dir ? 0 : e.size), 0);
  $('list-meta').textContent = entries.length
    ? `${entries.length} 项${dirs ? ` · ${dirs} 个文件夹` : ''} · ${formatSize(bytes)}`
    : '空文件夹';

  syncSortUI();
  updateSelectionUI();
}

function buildRow(entry) {
  const tr = document.createElement('tr');
  if (state.selected.has(entry.name)) tr.classList.add('selected');

  // 勾选
  const tdCheck = document.createElement('td');
  tdCheck.className = 'cell-check';
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
  const checkHit = document.createElement('label');
  checkHit.className = 'check-hit';
  checkHit.append(cb);
  tdCheck.append(checkHit);
  tr.append(tdCheck);

  // 名称
  const tdName = document.createElement('td');
  tdName.className = 'cell-name';
  const cell = document.createElement('div');
  cell.className = `name-cell${entry.is_dir ? ' is-dir' : ''}`;

  const kind = entry.is_dir ? 'folder' : fileKind(entry.name);
  const iconWrap = document.createElement('span');
  iconWrap.className = `file-icon ${kind}`;
  iconWrap.append(svgIcon(entry.is_dir ? ICON_DIR : (FILE_ICONS[kind] || ICON_FILE)));
  cell.append(iconWrap);

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
    badge.textContent = '共享目录外';
    badge.title = '这是一个指向共享目录外的符号链接，不能访问';
    cell.append(badge);
  }
  tdName.append(cell);
  tr.append(tdName);

  // 大小
  const tdSize = document.createElement('td');
  tdSize.className = `muted cell-size${entry.is_dir ? ' is-dir' : ''}`;
  tdSize.textContent = entry.is_dir ? '—' : formatSize(entry.size);
  tr.append(tdSize);

  // 时间
  const tdTime = document.createElement('td');
  tdTime.className = 'muted cell-time';
  tdTime.textContent = formatTime(entry.mtime);
  tr.append(tdTime);

  // 操作
  const tdAct = document.createElement('td');
  tdAct.className = 'cell-act';
  const actions = document.createElement('div');
  actions.className = 'row-actions';
  if (entry.restricted) {
    // 受限条目什么都不能做，不给按钮比给了再报错更清楚。
  } else if (entry.is_dir) {
    actions.append(rowButton('打包下载', ICON_ZIP, () => downloadZip([entry.name])));
  } else {
    actions.append(rowButton('下载', ICON_DOWNLOAD, () => downloadOne(entry.name)));
  }
  if (state.canWrite) {
    actions.append(
      rowButton('删除', ICON_TRASH, () => removePaths([entry.name]), { danger: true }),
    );
  }
  tdAct.append(actions);
  tr.append(tdAct);

  return tr;
}

function rowButton(label, icon, onClick, { danger = false } = {}) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = `btn btn-ghost btn-sm${danger ? ' btn-danger' : ''}`;
  b.title = label;
  b.setAttribute('aria-label', label);
  b.append(svgIcon(icon));
  const text = document.createElement('span');
  text.className = 'btn-label';
  text.textContent = label;
  b.append(text);
  b.addEventListener('click', onClick);
  return b;
}

function updateSelectionUI() {
  const n = state.selected.size;
  const has = n > 0;

  $('download-btn').disabled = !has;
  $('delete-btn').disabled = !has;
  const downloadLabel = $('download-btn').querySelector('.btn-label');
  if (downloadLabel) downloadLabel.textContent = has ? `下载所选（${n}）` : '下载所选';
  $('download-btn').setAttribute('aria-label', has ? `下载所选，已选 ${n} 项` : '下载所选');
  $('delete-btn').setAttribute('aria-label', has ? `删除所选，已选 ${n} 项` : '删除所选');

  $('selection-bar').hidden = !has;
  $('selection-count').textContent = `已选 ${n} 项`;

  const all = state.entries.length > 0 && n === state.entries.length;
  $('check-all').checked = all;
  $('check-all').indeterminate = has && !all;
}

function syncSortUI() {
  const select = $('sort-select');
  if (select) select.value = `${state.sort.key}:${state.sort.dir}`;
  for (const th of document.querySelectorAll('th[data-sort]')) {
    th.removeAttribute('aria-sort');
    if (th.dataset.sort === state.sort.key) {
      th.setAttribute('aria-sort', state.sort.dir === 1 ? 'ascending' : 'descending');
    }
  }
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
    syncSortUI();
    renderList();
  });
}

$('sort-select').addEventListener('change', (ev) => {
  const [key, dir] = ev.target.value.split(':');
  state.sort = { key, dir: Number(dir) };
  syncSortUI();
  renderList();
});

$('refresh-btn').addEventListener('click', async () => {
  const btn = $('refresh-btn');
  btn.classList.add('spinning');
  await refresh();
  setTimeout(() => btn.classList.remove('spinning'), 600);
});

$('selection-download').addEventListener('click', () => $('download-btn').click());
$('selection-delete').addEventListener('click', () => $('delete-btn').click());
$('selection-clear').addEventListener('click', () => {
  state.selected.clear();
  renderList();
});

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
  const ok = await uiConfirm(`确定要删除 ${label} 吗？此操作不可撤销。`, {
    title: '删除确认',
    confirmText: '删除',
    danger: true,
  });
  if (!ok) return;
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
  const name = await uiPrompt('新建文件夹', {
    message: '在当前目录下创建一个新文件夹。',
    placeholder: '文件夹名称',
    confirmText: '创建',
  });
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

// iOS Safari 不支持 webkitdirectory；留着一个点了没反应的按钮比藏起来更糟。
if (!('webkitdirectory' in document.createElement('input'))) {
  $('upload-folder').hidden = true;
}

syncSortUI();

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
  updateTransferTitle();
  return t;
}

function updateTransferTitle() {
  $('transfers-title').textContent = transfers.size
    ? `传输中 · ${transfers.size}`
    : '传输完成';
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
  updateTransferTitle();
  schedulePanelCleanup();
  refresh();
}

function failTransfer(t, message) {
  t.li.classList.add('error');
  t.statEl.textContent = message;
  transfers.delete(t.id);
  updateTransferTitle();
  schedulePanelCleanup();
  toast(message, 'error');
}

/** 全部传完后自动收起面板；还有任务就留着。 */
function schedulePanelCleanup() {
  setTimeout(() => {
    if (transfers.size !== 0) return;
    const panel = $('transfers');
    panel.hidden = true;
    panel.classList.remove('collapsed');
    $('transfers-toggle').textContent = '收起';
    // 这一批的结果已经展示过了，清掉，下一批从干净的面板开始。
    $('transfer-list').replaceChildren();
    $('transfers-title').textContent = '传输中';
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


// ─────────────────────────── 保存到本机目录 ───────────────────────────
//
// File System Access API 只在安全上下文（HTTPS 或 localhost）里可用。
// 局域网 HTTP 页面里 showDirectoryPicker 不存在，所以按钮默认隐藏，
// 并给出一条 HTTPS 提示。目录句柄存 IndexedDB，刷新后还能继续用。

const LOCAL_HANDLE_DB = 'wiretoss-local-save';
const LOCAL_HANDLE_STORE = 'handles';
const LOCAL_HANDLE_KEY = 'save-dir';

function localSaveSupported() {
  return window.isSecureContext && typeof window.showDirectoryPicker === 'function';
}

function openLocalHandleDb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(LOCAL_HANDLE_DB, 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(LOCAL_HANDLE_STORE)) db.createObjectStore(LOCAL_HANDLE_STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function loadStoredDirHandle() {
  try {
    const db = await openLocalHandleDb();
    return await new Promise((resolve) => {
      const tx = db.transaction(LOCAL_HANDLE_STORE, 'readonly');
      const req = tx.objectStore(LOCAL_HANDLE_STORE).get(LOCAL_HANDLE_KEY);
      req.onsuccess = () => resolve(req.result || null);
      req.onerror = () => resolve(null);
    });
  } catch {
    return null;
  }
}

async function storeDirHandle(handle) {
  try {
    const db = await openLocalHandleDb();
    await new Promise((resolve, reject) => {
      const tx = db.transaction(LOCAL_HANDLE_STORE, 'readwrite');
      tx.objectStore(LOCAL_HANDLE_STORE).put(handle, LOCAL_HANDLE_KEY);
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
    });
  } catch {
    // 隐私模式/IndexedDB 不可用时，本次会话仍然能用，只是下次要重选。
  }
}

async function ensureDirPermission(handle) {
  try {
    const opts = { mode: 'readwrite' };
    if ((await handle.queryPermission(opts)) === 'granted') return true;
    return (await handle.requestPermission(opts)) === 'granted';
  } catch {
    return false;
  }
}

function updateLocalDirUI() {
  const supported = localSaveSupported();
  $('save-local-btn').hidden = !supported;
  $('local-dir-btn').hidden = !supported;
  const label = state.localDirName ? `保存目录：${state.localDirName}` : '未设置保存目录';
  $('local-dir-label').textContent = label;
  $('local-dir-btn').title = state.localDirName
    ? `默认保存目录：${state.localDirName}（点击更改）`
    : '设置默认保存目录';
  $('save-local-btn').title = state.localDirName
    ? `保存到 ${state.localDirName}`
    : '保存到本机目录（第一次会先选择目录）';
}

function updateHttpsHint() {
  const hint = $('https-hint');
  const title = $('https-hint-title');
  const text = $('https-hint-text');
  const link = $('https-link');
  const cert = $('https-cert-link');

  if (localSaveSupported()) {
    hint.hidden = true;
    return;
  }

  hint.hidden = false;
  link.hidden = true;
  cert.hidden = true;

  if (!window.isSecureContext) {
    if (state.httpsEnabled && state.httpsPort) {
      title.textContent = '保存到本机目录需要 HTTPS';
      text.textContent = '当前页面不是浏览器认可的安全上下文（HTTP，或 HTTPS 证书尚未被信任）。请先下载并信任 CA 证书，然后用 HTTPS 地址打开。';
      link.hidden = false;
      link.href = `https://${location.hostname}:${state.httpsPort}/`;
      link.textContent = `用 HTTPS 打开（${location.hostname}:${state.httpsPort}）`;
      cert.hidden = false;
    } else {
      title.textContent = '保存到本机目录需要服务端启用 HTTPS';
      text.textContent = '当前服务只提供 HTTP。请在服务端的「服务设置」里启用 HTTPS，重启服务后刷新本页。';
    }
    return;
  }

  title.textContent = '当前浏览器不支持保存到本机目录';
  text.textContent = '这个功能需要桌面版 Chrome 或 Edge。Safari、Firefox 和手机浏览器不支持写本机文件夹；可以继续使用下载 / 打包下载。';
}

async function initLocalSave(me) {
  state.httpsEnabled = me.httpsEnabled === true;
  state.httpsPort = Number(me.httpsPort) || 0;
  updateHttpsHint();
  updateLocalDirUI();
  if (!localSaveSupported()) return;

  const stored = await loadStoredDirHandle();
  if (stored) {
    state.localDirHandle = stored;
    state.localDirName = stored.name || '';
    updateLocalDirUI();
  }
}

async function chooseLocalDir() {
  if (!localSaveSupported()) {
    toast('当前页面不能写本机目录，请用 HTTPS + 桌面 Chrome/Edge', 'error');
    return null;
  }
  try {
    const handle = await window.showDirectoryPicker({ id: 'wiretoss-save', mode: 'readwrite' });
    state.localDirHandle = handle;
    state.localDirName = handle.name || '';
    await storeDirHandle(handle);
    updateLocalDirUI();
    toast(`默认保存目录：${state.localDirName}`, 'ok');
    return handle;
  } catch (e) {
    if (e?.name !== 'AbortError') toast(`选择目录失败：${e.message || e}`, 'error');
    return null;
  }
}

function treeBytes(node) {
  if (!node.is_dir) return node.size || 0;
  return (node.children || []).reduce((sum, child) => sum + treeBytes(child), 0);
}

function countNewNode(node, plan) {
  if (node.is_dir) {
    plan.newDirs++;
    for (const child of node.children || []) countNewNode(child, plan);
  } else {
    plan.newFiles++;
  }
}

async function lookupEntry(dirHandle, name) {
  try {
    const handle = await dirHandle.getDirectoryHandle(name);
    return { kind: 'directory', handle };
  } catch { /* 可能是文件，也可能不存在 */ }
  try {
    const handle = await dirHandle.getFileHandle(name);
    return { kind: 'file', handle };
  } catch {
    return null;
  }
}

async function scanDirConflicts(node, destDir, plan, prefix) {
  for (const child of node.children || []) {
    const childPath = `${prefix}/${child.display_name}`;
    const existing = await lookupEntry(destDir, child.display_name);
    if (!existing) {
      countNewNode(child, plan);
    } else if (child.is_dir) {
      if (existing.kind === 'directory') {
        plan.existingDirs++;
        await scanDirConflicts(child, existing.handle, plan, childPath);
      } else {
        plan.typeConflicts.push(childPath);
      }
    } else if (existing.kind === 'file') {
      plan.overwrites.push(childPath);
    } else {
      plan.typeConflicts.push(childPath);
    }
  }
}

async function planLocalSave(nodes, destRoot) {
  const plan = {
    topLevelExisting: [],
    overwrites: [],
    typeConflicts: [],
    existingDirs: 0,
    newFiles: 0,
    newDirs: 0,
    totalBytes: 0,
  };
  for (const node of nodes) {
    plan.totalBytes += treeBytes(node);
    const existing = await lookupEntry(destRoot, node.display_name);
    if (!existing) {
      countNewNode(node, plan);
      continue;
    }
    plan.topLevelExisting.push(node.display_name);
    if (node.is_dir && existing.kind === 'directory') {
      plan.existingDirs++;
      await scanDirConflicts(node, existing.handle, plan, node.display_name);
    } else if (!node.is_dir && existing.kind === 'file') {
      plan.overwrites.push(node.display_name);
    } else {
      plan.typeConflicts.push(node.display_name);
    }
  }
  return plan;
}

let conflictResolve = null;
let conflictPreviousFocus = null;

function finishConflict(value) {
  const root = $('conflict-root');
  root.hidden = true;
  root.onkeydown = null;
  document.getElementById('app-view').inert = false;
  $('conflict-cancel').onclick = null;
  $('conflict-replace').onclick = null;
  $('conflict-merge').onclick = null;
  root.querySelector('[data-conflict-cancel]').onclick = null;
  if (conflictPreviousFocus?.focus) conflictPreviousFocus.focus();
  const resolve = conflictResolve;
  conflictResolve = null;
  resolve?.(value);
}

function showConflictDialog(plan) {
  const root = $('conflict-root');
  const app = $('app-view');
  const details = $('conflict-details');
  const summary = $('conflict-summary');
  const overwrite = plan.overwrites.length + plan.typeConflicts.length;
  const added = plan.newFiles;
  const typeNote = plan.typeConflicts.length
    ? '（会先删除目标中对应的文件或文件夹）'
    : '';
  summary.textContent = overwrite
    ? `目标目录已有 ${plan.topLevelExisting.length} 个同名项。合并将覆盖 ${plan.overwrites.length} 个文件、替换 ${plan.typeConflicts.length} 个类型冲突项${typeNote}，并新增 ${added} 个文件。`
    : `目标目录已有 ${plan.topLevelExisting.length} 个同名项。合并会保留已有文件并新增 ${added} 个文件；也可以删除同名项后完整复制。`;

  details.replaceChildren();
  const addDetail = (text) => {
    if (details.children.length >= 100) return false;
    const li = document.createElement('li');
    li.textContent = text;
    details.append(li);
    return true;
  };
  for (const name of plan.topLevelExisting) addDetail(`同名项：${name}`);
  for (const path of plan.overwrites) addDetail(`覆盖文件：${path}`);
  for (const path of plan.typeConflicts) addDetail(`类型冲突：${path}`);
  if (details.children.length >= 100) {
    const li = document.createElement('li');
    li.textContent = '……更多条目已省略';
    details.append(li);
  }
  details.hidden = details.children.length === 0;

  conflictPreviousFocus = document.activeElement;
  app.inert = true;
  root.hidden = false;

  return new Promise((resolve) => {
    conflictResolve = resolve;
    $('conflict-cancel').onclick = () => finishConflict(null);
    $('conflict-replace').onclick = () => finishConflict('replace');
    $('conflict-merge').onclick = () => finishConflict('merge');
    root.querySelector('[data-conflict-cancel]').onclick = () => finishConflict(null);
    root.onkeydown = (ev) => {
      if (ev.key === 'Escape') {
        ev.preventDefault();
        finishConflict(null);
      }
    };
    setTimeout(() => $('conflict-merge').focus(), 0);
  });
}

async function removeIfExists(parentDir, name) {
  const existing = await lookupEntry(parentDir, name);
  if (!existing) return;
  await parentDir.removeEntry(name, { recursive: true });
}

async function copyLocalNode(node, parentDir, name, transfer, progress, total) {
  if (node.is_dir) {
    let dirHandle;
    const existing = await lookupEntry(parentDir, name);
    if (existing?.kind === 'directory') {
      dirHandle = existing.handle;
    } else {
      if (existing) await parentDir.removeEntry(name, { recursive: true });
      dirHandle = await parentDir.getDirectoryHandle(name, { create: true });
    }
    for (const child of node.children || []) {
      progress = await copyLocalNode(
        child, dirHandle, child.display_name, transfer, progress, total,
      );
    }
    return progress;
  }

  const existing = await lookupEntry(parentDir, name);
  if (existing?.kind === 'directory') {
    await parentDir.removeEntry(name, { recursive: true });
  }
  const fileHandle = await parentDir.getFileHandle(name, { create: true });
  const res = await fetch(`/api/download?path=${encodeURIComponent(node.path)}`, { cache: 'no-store' });
  if (res.status === 401) {
    showLogin();
    throw new Error('登录已过期');
  }
  if (!res.ok) throw new Error(`下载「${node.path}」失败（HTTP ${res.status}）`);

  const writable = await fileHandle.createWritable();
  try {
    const reader = res.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      await writable.write(value);
      progress += value.length;
      updateTransfer(transfer, Math.min(progress, total), total);
    }
    await writable.close();
  } catch (e) {
    try { await writable.abort(); } catch { /* 忽略 */ }
    throw e;
  }
  return progress;
}

async function saveSelection(destRoot) {
  if (state.localSaveBusy) return;
  const selected = state.entries.filter((entry) => state.selected.has(entry.name));
  if (!selected.length) {
    toast('先选择要保存的文件或文件夹', 'error');
    return;
  }
  const skipped = selected.filter((entry) => entry.restricted);
  const usable = selected.filter((entry) => !entry.restricted);
  if (!usable.length) {
    toast('选中的都是共享目录外链接，无法保存', 'error');
    return;
  }
  if (skipped.length) toast(`已跳过 ${skipped.length} 个共享目录外链接`, 'error');

  state.localSaveBusy = true;
  let transfer = null;
  try {
    const nodes = [];
    for (const entry of usable) {
      const path = joinPath(state.cwd, entry.name);
      if (entry.is_dir) {
        const data = await api(`/api/tree?path=${encodeURIComponent(path)}`);
        nodes.push({
          display_name: entry.display_name,
          path,
          is_dir: true,
          size: 0,
          children: data.entries || [],
        });
      } else {
        nodes.push({
          display_name: entry.display_name,
          path,
          is_dir: false,
          size: entry.size,
          children: [],
        });
      }
    }

    const plan = await planLocalSave(nodes, destRoot);
    let strategy = 'merge';
    if (plan.topLevelExisting.length > 0) {
      strategy = await showConflictDialog(plan);
      if (!strategy) return;
      if (strategy === 'replace') {
        const names = plan.topLevelExisting.map((name) => `「${name}」`).join('、');
        const ok = await uiConfirm(
          `将先删除目标目录中的 ${names} 及其全部内容，再完整复制。此操作不可撤销。`,
          { title: '删除同名项', confirmText: '删除并复制', danger: true },
        );
        if (!ok) return;
      }
    }

    const total = plan.totalBytes;
    transfer = createTransfer('保存到本机', total);
    let progress = 0;
    for (const node of nodes) {
      if (strategy === 'replace') {
        await removeIfExists(destRoot, node.display_name);
      }
      progress = await copyLocalNode(
        node, destRoot, node.display_name, transfer, progress, total,
      );
    }
    finishTransfer(transfer);
    toast(`已保存到「${destRoot.name}」`, 'ok');
    transfer = null;
  } catch (e) {
    if (transfer) failTransfer(transfer, e.message || String(e));
    else if (e instanceof AuthError) showLogin();
    else toast(e.message || String(e), 'error');
  } finally {
    state.localSaveBusy = false;
  }
}

$('save-local-btn').addEventListener('click', async () => {
  if (!localSaveSupported()) {
    toast('当前页面不能写本机目录，请用 HTTPS + 桌面 Chrome/Edge', 'error');
    return;
  }
  if (!state.selected.size) {
    await chooseLocalDir();
    return;
  }
  let handle = state.localDirHandle;
  if (!handle) {
    handle = await chooseLocalDir();
    if (!handle) return;
  }
  if (!(await ensureDirPermission(handle))) {
    toast('没有写入该目录的权限', 'error');
    return;
  }
  await saveSelection(handle);
});

$('local-dir-btn').addEventListener('click', () => {
  void chooseLocalDir();
});

// ─────────────────────────────── 启动 ───────────────────────────────

boot();
