import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { createPages } from './pages.mjs';

// Preview aislada: todos los datos de este archivo son ficticios.
const previewDir = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(previewDir, '../..');
const read = (file) => fs.readFileSync(path.join(repo, file), 'utf8');

const slots = {
  GREETING: 'Buenas tardes, Alex',
  TODAYLINE: 'Viernes, 23 de octubre · Trimestre 2',
  SYNC: 'Mikareno: sincronizado ahora',
  CLASSCOUNT: '<span class="w-1.5 h-1.5 rounded-full bg-secondary"></span> 2 clases rest.',
  CLASSES: `
    <div class="p-space-sm rounded-lg bg-surface-container flex flex-col gap-1.5"><div><span class="font-code-inline text-[11px] text-primary-fixed-dim px-1.5 py-0.5 rounded bg-primary-container/10">09:00 - 10:30</span></div><h3 class="font-label-md text-label-md text-on-surface font-semibold">Algoritmos y estructuras de datos</h3></div>
    <div class="p-space-sm rounded-lg bg-surface-container flex flex-col gap-1.5"><div><span class="font-code-inline text-[11px] text-primary-fixed-dim px-1.5 py-0.5 rounded bg-primary-container/10">11:00 - 12:30</span></div><h3 class="font-label-md text-label-md text-on-surface font-semibold">Redes y sistemas distribuidos</h3></div>`,
  TASKCOUNT: '2 pendientes',
  TASKTODAY: `
    <div class="p-2 rounded-lg bg-surface-container flex items-start gap-2.5"><input class="mt-1 w-4 h-4 rounded accent-secondary" type="checkbox"/><div class="flex-1 min-w-0"><p class="font-label-md text-label-md text-on-surface font-medium truncate">Laboratorio 4: tiempos de ejecución</p><span class="font-body-sm text-[10px] text-outline">Algoritmos · 23:59</span></div></div>
    <div class="p-2 rounded-lg bg-surface-container flex items-start gap-2.5"><input class="mt-1 w-4 h-4 rounded accent-secondary" type="checkbox"/><div class="flex-1 min-w-0"><p class="font-label-md text-label-md text-on-surface font-medium truncate">Cuestionario previo de redes</p><span class="font-body-sm text-[10px] text-outline">Redes · 18:00</span></div></div>`,
  NOTES: [
    'Arquitectura Von Neumann',
    'Protocolos TCP vs UDP',
    'Grafos y recorridos Dijkstra',
  ].map((note) => `<div class="p-2.5 rounded-lg bg-surface-container flex items-center justify-between"><div class="flex items-start gap-2 min-w-0"><span class="material-symbols-outlined text-[18px] text-tertiary-fixed-dim">draft</span><a href="/dashboard/notes" class="font-code-inline text-[12px] text-primary-fixed-dim truncate">${note}</a></div><span class="font-code-inline text-[10px] px-1.5 py-0.5 rounded bg-surface-container-lowest text-outline">nota</span></div>`).join(''),
  AI_MODEL: 'Modelo: MiniMax M3',
  AI_RESUME: `<div class="p-3 rounded-lg bg-surface-container mb-space-sm"><div class="flex items-start gap-2.5"><div class="w-7 h-7 rounded-lg bg-primary-container/20 flex items-center justify-center shrink-0 text-primary-container"><span class="material-symbols-outlined text-[16px]">auto_awesome</span></div><div class="min-w-0 flex-1"><span class="font-label-sm text-[10px] uppercase tracking-wider text-outline">Asistente académico</span><p class="font-label-md text-label-md text-on-surface font-medium mt-0.5">Pregunta lo que necesites sobre tus materias.</p><div class="flex items-center gap-2 mt-2"><a href="/dashboard/ai" class="px-2.5 py-1 rounded bg-primary-container text-on-primary font-label-sm text-label-sm font-semibold">Abrir asistente</a></div></div></div></div>`,
  TASKTABLE: [
    ['Implementación HeapSort en Rust', 'Algoritmos', 'Hoy · 23:59', 'Alta'],
    ['Entrega proyecto: Routing RIP v2', 'Redes', 'Mañana', 'Media'],
    ['Ensayo: paralelismo y concurrencia', 'Sistemas Op.', '28 oct.', 'Media'],
    ['Lectura: paginación y memoria virtual', 'Sistemas Op.', '30 oct.', 'Baja'],
  ].map((task) => `<tr class="hover:bg-surface-container"><td class="py-2.5 pl-1 align-middle"><input class="w-3.5 h-3.5 rounded accent-secondary" type="checkbox"/></td><td class="py-2.5 pr-2 align-middle font-medium text-on-surface min-w-[140px]"><div class="truncate font-label-md text-label-sm">${task[0]}</div><span class="text-[10px] font-code-inline text-primary-fixed-dim">Pendiente</span></td><td class="py-2.5 pr-2 align-middle whitespace-nowrap text-outline">${task[1]}</td><td class="py-2.5 pr-2 align-middle whitespace-nowrap"><span class="font-code-inline text-[11px] text-outline">${task[2]}</span></td><td class="py-2.5 pr-2 align-middle whitespace-nowrap text-outline">${task[3]}</td><td class="py-2.5 pr-1 align-middle text-right"><span class="material-symbols-outlined text-[16px] text-on-surface-variant">more_vert</span></td></tr>`).join(''),
  TASKFOOTER: 'Mostrando 4 de 11 tareas activas',
  WEEKTITLE: 'Semana 43',
  WEEKCAL: ['L|19', 'M|20', 'X|21', 'J|22', 'V|23', 'S|24', 'D|25'].map((value, index) => {
    const [letter, day] = value.split('|');
    const today = index === 4;
    return `<div class="flex flex-col items-center p-1.5 rounded-lg ${today ? 'bg-primary-container/15 text-primary-container shadow-inner' : 'bg-surface-container/60'}"><span class="font-label-sm text-[10px] ${today ? 'font-bold text-primary-container' : 'text-outline'}">${letter}</span><span class="font-code-inline text-code-inline ${today ? 'text-primary-container' : 'text-on-surface'} font-semibold mt-0.5">${day}</span><div class="flex gap-0.5 mt-1.5">${index < 5 ? '<span class="w-1 h-1 rounded-full bg-secondary"></span>' : '<span class="h-1"></span>'}${today ? '<span class="w-1.5 h-1.5 rounded-full bg-error"></span>' : ''}</div></div>`;
  }).join(''),
  EXAM: `<div class="rounded-xl bg-surface-container-low p-space-md relative overflow-hidden"><div class="flex items-center justify-between mb-2"><span class="px-2 py-0.5 rounded text-[10px] font-code-inline bg-error-container/40 text-error font-semibold flex items-center gap-1"><span class="material-symbols-outlined text-[12px]">timer</span> PRÓXIMO EXAMEN</span><span class="font-code-inline text-[11px] text-outline">28 oct. · 08:30</span></div><h3 class="font-headline-sm text-headline-sm text-on-surface font-bold">Sistemas Operativos: concurrencia y kernel</h3><p class="mt-2 text-[10px] text-outline">Auditorio central · Capítulos 1 al 5</p></div>`,
  FOCUS: `<div class="rounded-xl bg-surface-container-low p-space-md flex flex-col"><div class="flex items-center justify-between pb-space-xs mb-space-xs"><span class="font-label-sm text-[10px] uppercase text-outline tracking-wider font-semibold">Promedio por materia</span><span class="font-code-inline text-[11px] text-outline">Trimestre 2</span></div><div class="space-y-1.5 mt-1">${[['Algoritmos y complejidad', '8.70', 87, 'bg-primary-container'], ['Redes de computadoras', '8.20', 82, 'bg-primary-container'], ['Sistemas operativos', '6.70', 67, 'bg-secondary']].map(([name, grade, percent, color]) => `<div class="flex items-center justify-between text-body-sm text-[11px]"><span class="text-on-surface-variant truncate">${name}</span><span class="font-code-inline text-on-surface ml-2">${grade}</span></div><div class="w-full h-1.5 rounded-full bg-surface-container overflow-hidden mb-1.5"><div class="h-full ${color} rounded-full" style="width:${percent}%"></div></div>`).join('')}</div><div class="mt-space-sm pt-space-xs flex justify-end"><a href="/dashboard/subjects" class="font-label-sm text-[11px] text-outline flex items-center gap-1"><span class="material-symbols-outlined text-[14px]">query_stats</span> Ver asignaturas</a></div></div>`,
  SUBJECT_OPTIONS: '<option>Algoritmos</option><option>Redes</option><option>Sistemas operativos</option>',
};

let dashboard = read('static/stitch-v2/_content/clean/dashboard.html');
for (const [key, value] of Object.entries(slots)) dashboard = dashboard.replaceAll(`<!--SLOT:${key}-->`, value);

let aside = read('static/stitch-v2/shell/aside.html')
  .replace('<aside ', '<aside id="app-rail" ')
  .replace('title="Colapsar menú"', 'id="rail-toggle-aside" type="button" title="Colapsar menú" aria-controls="app-rail" aria-expanded="true"')
  .replace('<!--SLOT:SHELL_NAV_MCP-->', '3 integradas')
  .replace('<!--SLOT:SHELL_VAULT_PCT-->', '1.5');

const avatar = '<span class="w-8 h-8 rounded-full bg-primary-container/20 ring-1 ring-primary/40 flex items-center justify-center font-label-md text-[11px] text-primary-container font-semibold">AL</span>';
let topbar = read('static/stitch-v2/shell/topbar.html')
  .replace('<header ', '<header id="app-topbar" ')
  .replace(/<div class="app-topbar-left[^"]*">/, (opening) => `${opening}<button id="rail-toggle" type="button" aria-controls="app-rail" aria-expanded="true" aria-label="Mostrar u ocultar la navegación"><span class="material-symbols-outlined">menu</span></button>`)
  .replaceAll('<!--SLOT:SHELL_AVATAR-->', avatar)
  .replaceAll('<!--SLOT:SHELL_NAME-->', 'Alex')
  .replaceAll('<!--SLOT:SHELL_ROLE-->', 'Ingeniería')
  .replace('<!--SLOT:SHELL_STATUS-->', '<span class="w-2 h-2 rounded-full bg-secondary animate-pulse"></span><span class="font-label-sm text-label-sm text-on-surface-variant">En línea · Mikareno conectado</span>');

const head = read('static/stitch-v2/shell/head.html')
  .replace('https://cdn.tailwindcss.com', '/node_modules/tailwindcss-cdn/tailwindcss.js')
  .replace('https://unpkg.com/htmx.org@2.0.3/dist/htmx.min.js', '/node_modules/htmx.org/dist/htmx.min.js')
  .replace(/<link rel="preconnect"[^>]+>/g, '')
  .replace(/<link href="https:\/\/fonts\.googleapis\.com[^>]+>/, '<link rel="stylesheet" href="/node_modules/@fontsource/dm-sans/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/space-grotesk/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/manrope/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/material-symbols-outlined/latin-400.css">');
const rust = read('src/handlers/stitch_v2.rs');
const shellCss = rust.match(/const SHELL_CSS: &str = r##"([\s\S]*?)"##;/)?.[1] || '';
const shellJs = rust.match(/const SHELL_JS: &str = r##"([\s\S]*?)"##;/)?.[1] || '';
const userMenuJs = rust.match(/const USER_MENU_JS: &str = r##"([\s\S]*?)"##;/)?.[1] || '';
const userButton = rust.match(/fn user_menu_button\(\)[\s\S]*?r##"([\s\S]*?)"##/)?.[1]
  .replaceAll('{USER_MENU_BUTTON_ID}', 'topProfile')
  .replaceAll('{USER_MENU_ID}', 'user-menu');
const panelSource = rust.match(/fn user_menu_panel[\s\S]*?fn inject_user_menu/)?.[0] || '';
const itemClass = panelSource.match(/const ITEM: &str = "([^"]+)"/)?.[1] || '';
const panelFragments = [...panelSource.matchAll(/r##"([\s\S]*?)"##/g)].map((match) => match[1]);
const accountAction = (panelFragments[0] || '').replaceAll('{ITEM}', itemClass);
const userPanel = (panelFragments[2] || '')
  .replaceAll('{USER_MENU_ID}', 'user-menu')
  .replaceAll('{ITEM}', itemClass)
  .replaceAll('{action}', accountAction);
const fillUserSlots = (value) => value
  .replaceAll('<!--SLOT:SHELL_AVATAR-->', avatar)
  .replaceAll('<!--SLOT:SHELL_NAME-->', 'Alex')
  .replaceAll('<!--SLOT:SHELL_ROLE-->', 'Ingeniería');
topbar = topbar.replace(
  /<div class="topbar-user[\s\S]*?expand_more<\/span>\s*<\/div>/,
  `<div class="relative">${fillUserSlots(userButton)}${fillUserSlots(userPanel)}</div>`,
);

const pages = createPages(dashboard);

// Autenticación: se extraen las plantillas reales de Rust y solo se sustituyen
// los slots que normalmente rellena el servidor. Así la preview no mantiene una
// segunda maqueta que pueda desincronizarse del acceso de producción.
const authSource = read('src/handlers/auth.rs');
const loginTemplate = authSource.match(/const LOGIN_PAGE_HTML: &str = r##"([\s\S]*?)"##;/)?.[1] || '';
const loginErrorHtml = authSource.match(/const LOGIN_ERROR_HTML: &str = r##"([\s\S]*?)"##;/)?.[1] || '';
const passwordSource = authSource.slice(authSource.indexOf('fn password_page_html('));
const passwordTemplate = passwordSource.match(/r##"(<!DOCTYPE html>[\s\S]*?)"##,/)?.[1] || '';
const passwordJs = authSource.match(/const PASSWORD_PAGE_JS: &str = r#"([\s\S]*?)"#;/)?.[1] || '';
const passwordMinLength = authSource.match(/const MIN_NEW_PASSWORD_LEN: usize = (\d+);/)?.[1] || '12';
const loginHead = loginTemplate.match(/<head>[\s\S]*?<\/head>/)?.[0] || '';
const passwordPage = passwordTemplate
  .replace('{head}', loginHead.replace('<title>NOVA — Iniciar sesión</title>', '<title>NOVA — Cambiar contraseña</title>'))
  .replace('{banner}', '')
  .replace('{username_input}', '<input id="username" type="text" name="username" value="alex" readonly autocomplete="username">')
  .replace('{action}', '/settings/password')
  .replaceAll('{min_len}', passwordMinLength)
  .replace('{js}', passwordJs);
const localizeAuth = (html) => html
  .replace('https://cdn.tailwindcss.com', '/node_modules/tailwindcss-cdn/tailwindcss.js')
  .replace('https://unpkg.com/htmx.org@1.9.12', '/node_modules/htmx.org/dist/htmx.min.js')
  .replace(/<link rel="preconnect"[^>]+>/g, '')
  .replace(/<link href="https:\/\/fonts\.googleapis\.com[^>]+>/, '<link rel="stylesheet" href="/node_modules/@fontsource/dm-sans/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/space-grotesk/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/manrope/latin.css"><link rel="stylesheet" href="/node_modules/@fontsource/material-symbols-outlined/latin-400.css">');
const authPages = new Map([
  ['/login', localizeAuth(loginTemplate)],
  ['/settings/password', localizeAuth(passwordPage)],
]);
const navSource = rust.match(/pub const NAV_LINKS[\s\S]*?=\s*&\[([\s\S]*?)\];/)?.[1] || '';
const navLinks = [...navSource.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((match) => [match[1], match[2]]);
const stateTokens = ['bg-primary-container', 'text-on-primary-container', 'font-semibold', 'text-on-surface-variant', 'hover:text-on-surface', 'hover:bg-surface-container', 'hover:bg-primary-fixed-dim'];
const activeClasses = ['text-on-primary-container', 'bg-primary-container', 'font-label-md', 'font-semibold', 'hover:bg-primary-fixed-dim'];
const idleClasses = ['text-on-surface-variant', 'hover:text-on-surface', 'hover:bg-surface-container'];
const asideFor = (route) => {
  const section = [
    ['/dashboard/calendar/', '/dashboard/calendar'],
    ['/dashboard/subjects/', '/dashboard/subjects'],
    ['/office/docs/', '/office/docs'],
    ['/office/sheets/', '/office/sheets'],
    ['/office/presentations/', '/office/presentations'],
    ['/office/library/', '/office/library'],
  ].find(([prefix]) => route.startsWith(prefix))?.[1] || route;
  const active = section === '/dashboard/notes'
    ? 'segundo-cerebro'
    : navLinks.find(([, target]) => target === section)?.[0];
  return aside.replace(/<a\b[^>]*data-path="([^"]+)"[^>]*>/g, (tag, key) => {
    const link = navLinks.find(([pathKey]) => pathKey === key);
    const isActive = key === active;
    let out = link ? tag.replace(/href="[^"]*"/, `href="${link[1]}"`) : tag;
    out = out.replace(/\s+aria-current="[^"]*"/g, '');
    out = out.replace(/class="([^"]*)"/, (_, classes) => {
      const base = classes.split(/\s+/).filter((name) => name && !stateTokens.includes(name));
      return `class="${[...base, ...(isActive ? activeClasses : idleClasses)].join(' ')}"`;
    });
    return isActive ? out.replace(/>$/, ' aria-current="page">') : out;
  });
};
const shell = (content, route) => `<!DOCTYPE html><html class="dark" lang="es"><head>${head}<style>${shellCss}</style></head><body class="bg-background font-body-md text-body-md text-on-surface antialiased"><a class="skip-link" href="#contenido">Saltar al contenido</a>${asideFor(route)}<button id="rail-backdrop" type="button" aria-label="Cerrar menú"></button><div id="app-rail-content" class="pl-[250px]">${topbar}<main id="contenido" tabindex="-1" class="relative pt-16 w-full min-h-screen bg-background">${content}</main></div><script>${shellJs}${userMenuJs}</script><script src="/shell.js" defer></script></body></html>`;
const types = { '.css': 'text/css; charset=utf-8', '.js': 'application/javascript; charset=utf-8', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff' };
const json = (res, value) => {
  res.writeHead(200, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' });
  res.end(JSON.stringify(value));
};
const readJson = (req) => new Promise((resolve) => {
  let raw = '';
  req.setEncoding('utf8');
  req.on('data', (chunk) => { if (raw.length < 16384) raw += chunk; });
  req.on('end', () => {
    try { resolve(JSON.parse(raw || '{}')); } catch { resolve({}); }
  });
  req.on('error', () => resolve({}));
});
const previewRag = (mode) => {
  const noteSources = mode === 'hybrid' || mode === 'notes' ? [{
    type: 'note', title: 'Concurrencia y semáforos', url: '/app/notes/concurrencia',
    snippet: 'Un semáforo coordina el acceso a recursos compartidos.',
  }] : [];
  const webSources = mode === 'hybrid' ? [{
    type: 'web', title: 'Semáforo (informática) — Wikipedia',
    url: 'https://es.wikipedia.org/wiki/Sem%C3%A1foro_(inform%C3%A1tica)',
    snippet: 'Referencia web ficticia para comprobar la presentación de citas.',
  }] : [];
  return { mode, web: mode === 'hybrid', noteSources, webSources };
};

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname === '/health') {
    json(res, { status: 'ok', components: { openrouter: true, mikareno: true } });
    return;
  }
  if (url.pathname === '/api/notes') {
    json(res, { notes: [
      { slug: 'concurrencia', title: 'Concurrencia y semáforos', tags: 'sistemas' },
      { slug: 'routing', title: 'Routing dinámico', tags: 'redes' },
    ] });
    return;
  }
  if (url.pathname.startsWith('/api/notes/')) {
    const slug = decodeURIComponent(url.pathname.slice('/api/notes/'.length));
    json(res, { note: { slug, title: slug === 'routing' ? 'Routing dinámico' : 'Concurrencia y semáforos', tags: 'sistemas' }, content: '# Concurrencia y semáforos\n\nUn **semáforo** coordina el acceso a recursos compartidos.\n\n## Ideas clave\n\n- Exclusión mutua\n- Sincronización' });
    return;
  }
  if (url.pathname === '/api/ai/conversations') {
    json(res, { conversations: [
      { subject: 'sistemas', last_message: 'Un semáforo coordina procesos.', message_count: 6, updated_at: '2026-10-23T16:30:00Z' },
      { subject: 'redes', last_message: 'RIP utiliza distancia administrativa.', message_count: 4, updated_at: '2026-10-22T14:10:00Z' },
    ] });
    return;
  }
  if (/^\/api\/ai\/conversations\/[^/]+\/messages$/.test(url.pathname)) {
    json(res, { messages: [
      { role: 'user', content: '¿Qué es un semáforo?', created_at: '2026-10-23T16:29:00Z' },
      { role: 'assistant', content: 'Es un mecanismo de sincronización que coordina el acceso a recursos compartidos.', created_at: '2026-10-23T16:30:00Z' },
    ] });
    return;
  }
  if (url.pathname === '/api/ai/models') {
    json(res, { active: 'minimax/minimax-m2:free', models: [
      { id: 'minimax/minimax-m2:free', name: 'MiniMax M2' },
      { id: 'google/gemma-4-31b-it:free', name: 'Gemma 4 31B' },
    ] });
    return;
  }
  if (url.pathname === '/api/ai/cost') {
    json(res, { total_cost: 0.42, budget_limit: 25 });
    return;
  }
  if (url.pathname === '/api/ai/summary' && req.method === 'POST') {
    json(res, { summary: 'La concurrencia coordina procesos y los semáforos protegen los recursos compartidos.' });
    return;
  }
  if (url.pathname === '/api/ai/quiz' && req.method === 'POST') {
    json(res, { questions: [{ question: '¿Qué problema resuelve un semáforo?', options: ['La sincronización', 'El routing'], answer: 'La sincronización' }] });
    return;
  }
  if (url.pathname === '/api/ai/study-sheet' && req.method === 'POST') {
    json(res, { study_sheet: 'Pregunta: ¿para qué sirve un semáforo?\nRespuesta: para coordinar el acceso a recursos compartidos.' });
    return;
  }
  if (url.pathname === '/api/ai/chat/stream' && req.method === 'POST') {
    const body = await readJson(req);
    const mode = ['hybrid', 'notes', 'general'].includes(body.mode) ? body.mode : 'general';
    const rag = previewRag(mode);
    res.writeHead(200, { 'content-type': 'text/event-stream; charset=utf-8', 'cache-control': 'no-store' });
    res.write(`event: context\ndata: ${JSON.stringify({ mode, web: rag.web, sources: rag.noteSources })}\n\n`);
    res.write('event: delta\ndata: {"content":"Respuesta ficticia de la preview: repasa la idea y contrástala con tus apuntes."}\n\n');
    setTimeout(() => {
      if (rag.webSources.length) res.write(`event: sources\ndata: ${JSON.stringify({ sources: rag.webSources })}\n\n`);
      res.write('event: done\ndata: {"usage":{"input":18,"output":24,"cost":0.001}}\n\n');
      res.end();
    }, 30);
    return;
  }
  if (url.pathname === '/api/ai/chat' && req.method === 'POST') {
    const body = await readJson(req);
    const mode = ['hybrid', 'notes', 'general'].includes(body.mode) ? body.mode : 'general';
    const rag = previewRag(mode);
    json(res, {
      content: 'Respuesta ficticia de la preview: repasa la idea y contrástala con tus apuntes.',
      mode,
      sources: [...rag.noteSources, ...rag.webSources],
    });
    return;
  }
  if (url.pathname === '/dashboard/calendar' && url.searchParams.get('partial') === 'grid') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end('<div id="calendar-grid" class="nova-calendar-grid-host"><p class="nova-card-intro">Mes actualizado · datos ficticios</p></div>');
    return;
  }
  if (url.pathname === '/login' && req.method === 'POST') {
    req.resume();
    res.writeHead(401, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    res.end(loginErrorHtml);
    return;
  }
  if (authPages.has(url.pathname) && req.method === 'GET') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    res.end(authPages.get(url.pathname));
    return;
  }
  const pageRoute = url.pathname === '/app/notes/concurrencia' ? '/dashboard/notes' : url.pathname;
  if (pages.has(pageRoute)) {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    res.end(shell(pages.get(pageRoute), url.pathname));
    return;
  }
  const assets = {
    '/polish.css': 'static/app/polish.css',
    '/nova.css': 'static/app/nova.css',
    '/shell.js': 'static/app/shell.js',
    '/favicon.svg': 'static/app/favicon.svg',
  };
  let file;
  if (assets[url.pathname]) file = path.join(repo, assets[url.pathname]);
  if (url.pathname.startsWith('/node_modules/')) file = path.join(previewDir, 'node_modules', decodeURIComponent(url.pathname.slice('/node_modules/'.length)));
  if (!file || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('Ruta no incluida en la preview Nova.');
    return;
  }
  res.writeHead(200, { 'content-type': types[path.extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' });
  fs.createReadStream(file).pipe(res);
});
server.listen(3000, '0.0.0.0', () => console.log(`Nova UI preview ready on 0.0.0.0:3000 · ${pages.size} páginas`));
