import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import chromium from './node_modules/@sparticuz/chromium/build/esm/index.js';
import { inflate } from './node_modules/@sparticuz/chromium/build/esm/lambdafs.js';
import { setupLambdaEnvironment } from './node_modules/@sparticuz/chromium/build/esm/helper.js';
import { createPages } from './pages.mjs';

const require = createRequire(import.meta.url);
const puppeteer = require('./node_modules/puppeteer-core');
await inflate(new URL('./node_modules/@sparticuz/chromium/bin/al2023.tar.br', import.meta.url).pathname);
setupLambdaEnvironment('/tmp/al2023/lib');
const browser = await puppeteer.launch({
  args: [...chromium.args, '--no-sandbox'],
  defaultViewport: null,
  executablePath: await chromium.executablePath(),
  headless: chromium.headless,
});

const base = 'http://127.0.0.1:3000';
const onlyRoutes = new Set((process.env.NOVA_ROUTES || '').split(',').filter(Boolean));
const onlyViewports = new Set((process.env.NOVA_VIEWPORTS || '').split(',').filter(Boolean));
const captureAll = process.env.NOVA_SCREENSHOTS === 'all';
const fullPage = process.env.NOVA_FULL_PAGE !== '0';
const appRoutes = [...createPages('dashboard').keys()].filter((route) => route !== '/');
const routes = [...appRoutes, '/login', '/settings/password'].filter((route) => !onlyRoutes.size || onlyRoutes.has(route));
const viewports = [
  { name: 'desktop', width: 1440, height: 1000 },
  { name: 'mobile', width: 390, height: 844 },
].filter((viewport) => !onlyViewports.size || onlyViewports.has(viewport.name));
const screenshots = new Set([
  '/dashboard', '/dashboard/material', '/dashboard/ai', '/ai', '/dashboard/notes',
  '/dashboard/mcp', '/admin', '/settings', '/dashboard/subjects', '/dashboard/calendar',
  '/dashboard/planner', '/dashboard/vida-escolar', '/dashboard/profile', '/dashboard/search',
  '/dashboard/subjects/demo', '/dashboard/subjects/missing',
  '/office', '/office/docs/demo', '/office/sheets/demo', '/office/presentations/demo',
  '/office/presentations/demo/present', '/office/library', '/login', '/settings/password',
]);
const results = [];
const failures = [];
const activeByRoute = {
  '/dashboard': 'panel-principal', '/dashboard/material': 'generador-de-material',
  '/settings': 'configuracion', '/dashboard/ai': 'asistente-academico', '/ai': 'conversaciones',
  '/dashboard/notes': 'segundo-cerebro', '/dashboard/mcp': 'centro-mcp', '/admin': 'administracion',
  '/dashboard/subjects': 'asignaturas', '/dashboard/planner': 'planificacion',
  '/dashboard/calendar': 'mi-agenda', '/dashboard/examenes': 'examenes',
  '/dashboard/horario': 'horarios', '/dashboard/vida-escolar': 'vida-escolar',
  '/dashboard/search': 'busqueda-global', '/office/docs': 'documentos',
  '/office/sheets': 'hojas-de-calculo', '/office/presentations': 'presentaciones',
  '/office/library': 'biblioteca-de-archivos',
};
const activeForRoute = (route) => {
  const section = [
    ['/dashboard/calendar/', '/dashboard/calendar'], ['/dashboard/subjects/', '/dashboard/subjects'],
    ['/office/docs/', '/office/docs'], ['/office/sheets/', '/office/sheets'],
    ['/office/presentations/', '/office/presentations'], ['/office/library/', '/office/library'],
  ].find(([prefix]) => route.startsWith(prefix))?.[1] || route;
  return activeByRoute[section] || null;
};

const settle = async (page, ms = 100) => {
  await new Promise((resolve) => setTimeout(resolve, ms));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
};

async function openPage(route, viewport, track = true) {
  const page = await browser.newPage();
  const errors = [];
  const badResponses = [];
  const external = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
  page.on('response', (response) => {
    if (response.url().startsWith(base) && response.status() >= 400) badResponses.push(`${response.status()} ${response.url()}`);
  });
  page.on('request', (request) => {
    const url = request.url();
    if (!url.startsWith(base) && !url.startsWith('data:') && !url.startsWith('blob:')) external.push(url);
  });
  await page.setViewport({ width: viewport.width, height: viewport.height, deviceScaleFactor: 1 });
  const response = await page.goto(`${base}${route}`, { waitUntil: 'networkidle0', timeout: 30000 });
  await page.evaluate(() => document.fonts.ready);
  await settle(page);
  if (track && response.status() !== 200) badResponses.push(`${response.status()} documento`);
  return { page, errors, badResponses, external };
}

for (const viewport of viewports) {
  for (const route of routes) {
    const { page, errors, badResponses, external } = await openPage(route, viewport);
    const isAuth = route === '/login' || route === '/settings/password';
    const metrics = await page.evaluate((authPage) => {
      const ids = [...document.querySelectorAll('[id]')].map((el) => el.id);
      const main = authPage ? document.querySelector('main') : document.querySelector('#contenido');
      const nova = authPage ? document.querySelector('.auth-shell') : document.querySelector('.nx-page, .nx-chat, .nx-notes-page, .nova-page, .nova-dashboard');
      const rail = document.querySelector('#app-rail');
      const overflowing = [...document.querySelectorAll(authPage ? 'body *' : '#contenido *')].map((el) => {
        const rect = el.getBoundingClientRect();
        return { tag: el.tagName, className: String(el.className).slice(0, 90), left: Math.round(rect.left), right: Math.round(rect.right) };
      }).filter((item) => item.right > innerWidth + 2 || item.left < -2).slice(0, 8);
      return {
        documentWidth: document.documentElement.scrollWidth,
        viewportWidth: innerWidth,
        navItems: document.querySelectorAll('#app-rail [data-path]').length,
        activeNav: document.querySelector('#app-rail [aria-current="page"]')?.dataset.path || null,
        activeNavCount: document.querySelectorAll('#app-rail [aria-current="page"]').length,
        menuVisibility: rail ? getComputedStyle(rail).visibility : 'hidden',
        duplicateIds: ids.filter((id, index) => ids.indexOf(id) !== index),
        slotsLeft: document.documentElement.innerHTML.includes('<!--SLOT:'),
        mainLength: main ? main.innerText.trim().length : 0,
        hasNova: Boolean(nova),
        authForm: authPage ? Boolean(document.querySelector('#login-container form')) : true,
        htmxLoaded: typeof window.htmx === 'function' || typeof window.htmx === 'object',
        versionVisible: document.body.textContent.includes('3.0'),
        overflowing,
      };
    }, isAuth);
    const record = { viewport: viewport.name, route, ...metrics, errors, badResponses, external };
    results.push(record);
    const problems = [];
    if (metrics.documentWidth !== metrics.viewportWidth) problems.push(`overflow ${metrics.documentWidth}/${metrics.viewportWidth}`);
    if (!isAuth) {
      if (metrics.navItems !== 22) problems.push(`nav=${metrics.navItems}`);
      const expectedActive = activeForRoute(route);
      if (metrics.activeNav !== expectedActive || metrics.activeNavCount !== (expectedActive ? 1 : 0)) problems.push(`activo=${metrics.activeNav}/${expectedActive} (${metrics.activeNavCount})`);
      const expectedMenuVisibility = viewport.name === 'desktop' ? 'visible' : 'hidden';
      if (metrics.menuVisibility !== expectedMenuVisibility) problems.push(`menú=${metrics.menuVisibility}/${expectedMenuVisibility}`);
    } else if (!metrics.authForm) problems.push('formulario de acceso ausente');
    if (metrics.duplicateIds.length) problems.push(`IDs=${metrics.duplicateIds.join(',')}`);
    if (metrics.slotsLeft) problems.push('slots pendientes');
    if (metrics.mainLength < 10) problems.push('contenido vacío');
    if (!metrics.hasNova) problems.push('sin composición Nova');
    if (!metrics.htmxLoaded) problems.push('HTMX no cargó');
    if (!metrics.versionVisible) problems.push('versión 3.0 ausente');
    if (errors.length) problems.push(`consola=${errors.join(' | ')}`);
    if (badResponses.length) problems.push(`HTTP=${badResponses.join(' | ')}`);
    if (external.length) problems.push(`externos=${external.join(' | ')}`);
    if (problems.length) {
      failures.push({ route, viewport: viewport.name, problems, metrics });
      console.error(`✗ ${viewport.name} ${route}: ${problems.join(' · ')}`);
    } else {
      console.log(`✓ ${viewport.name} ${route}`);
    }
    if (captureAll || screenshots.has(route)) {
      const slug = route === '/dashboard' ? 'dashboard' : route.replace(/^\//, '').replaceAll('/', '-');
      await page.screenshot({ path: `/tmp/nova-${slug}-${viewport.name}.png`, fullPage });
    }
    await page.close();
  }
}

async function interaction(name, route, callback, viewport = viewports[0], allowedErrorStatuses = []) {
  if (process.env.NOVA_INTERACTIONS === '0') return;
  const { page, errors, badResponses, external } = await openPage(route, viewport, false);
  try {
    await callback(page);
    await settle(page);
    const unexpectedErrors = errors.filter((message) => !allowedErrorStatuses.some((status) => message.includes(String(status))));
    assert.deepEqual(unexpectedErrors, [], `Consola limpia en ${name}`);
    const unexpectedResponses = badResponses.filter((item) => !allowedErrorStatuses.some((status) => item.startsWith(`${status} `)));
    assert.deepEqual(unexpectedResponses, [], `Sin respuestas HTTP erróneas en ${name}`);
    assert.deepEqual(external, [], `Sin recursos externos en ${name}`);
    console.log(`✓ interacción ${name}`);
    results.push({ interaction: name, route, ok: true });
  } catch (error) {
    failures.push({ interaction: name, route, problems: [error.stack || error.message], errors, badResponses, external });
    console.error(`✗ interacción ${name}: ${error.message}`);
  } finally {
    await page.close();
  }
}

await interaction('acceso: contraseña y error 401', '/login', async (page) => {
  assert.equal(await page.$eval('#password', (el) => el.type), 'password');
  await page.click('#toggle-password');
  assert.equal(await page.$eval('#password', (el) => el.type), 'text');
  assert.equal(await page.$eval('#toggle-password', (el) => el.getAttribute('aria-pressed')), 'true');
  await page.click('#toggle-password');
  assert.equal(await page.$eval('#password', (el) => el.type), 'password');
  await page.type('#username', 'usuario-demo');
  await page.type('#password', 'clave-incorrecta');
  await page.click('.auth-submit');
  await page.waitForSelector('#login-error [role="alert"]', { timeout: 4000 });
  assert.match(await page.$eval('#login-error', (el) => el.textContent), /Credenciales incorrectas/);
  assert.equal(await page.$eval('#username', (el) => el.value), 'usuario-demo');
}, viewports[0], [401]);

await interaction('contraseña: tres controles de visibilidad', '/settings/password', async (page) => {
  for (const id of ['current_password', 'new_password', 'confirm_password']) {
    assert.equal(await page.$eval(`#${id}`, (el) => el.type), 'password');
    await page.click(`#toggle-${id}`);
    assert.equal(await page.$eval(`#${id}`, (el) => el.type), 'text');
    assert.equal(await page.$eval(`#toggle-${id}`, (el) => el.getAttribute('aria-pressed')), 'true');
  }
});

await interaction('shell y menús', '/dashboard', async (page) => {
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'visible');
  assert.equal(await page.$eval('body', (el) => el.style.overflow), '');
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'hidden');
  assert.equal(await page.$eval('#rail-toggle', (el) => el.getAttribute('aria-expanded')), 'false');
  assert.equal(await page.$eval('body', (el) => el.style.overflow), '');
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'visible');
  assert.equal(await page.$eval('#rail-toggle', (el) => el.getAttribute('aria-expanded')), 'true');
  await page.click('#rail-toggle-aside');
  await settle(page);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'hidden');
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'visible');
  await page.click('#toggle-create-menu');
  assert.equal(await page.$eval('#create-modal-popover', (el) => el.classList.contains('hidden')), false);
  await page.click('#close-create-menu');
  await page.click('#quick-create-btn');
  assert.equal(await page.$eval('#quick-task-modal', (el) => el.classList.contains('hidden')), false);
  await page.click('#cancel-modal-btn');
  await page.click('#topProfile');
  assert.equal(await page.$eval('#user-menu', (el) => el.hidden), false);
  assert.equal(await page.$$eval('#user-menu a[href="/dashboard/profile"]', (links) => links.length), 1);
  await page.keyboard.press('Escape');
});

await interaction('cajón móvil', '/dashboard', async (page) => {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('body', (el) => el.classList.contains('rail-open')), true);
  const vaultBelow = await page.evaluate(() => document.querySelector('.nx-sidebar .app-vault').getBoundingClientRect().top >= document.querySelector('.nx-sidebar .app-system-nav').getBoundingClientRect().bottom - 1);
  assert.equal(vaultBelow, true);
}, viewports[1]);

await interaction('fuentes del asistente', '/dashboard/ai', async (page) => {
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), true);
  await page.click('#toggle-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), false);
  await page.click('#close-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), true);
  assert.equal(await page.$$eval('[data-chat-turn]', (els) => els.length), 2);
});

await interaction('chat principal: streaming sin envíos duplicados', '/dashboard/ai', async (page) => {
  let streamRequests = 0;
  page.on('request', (request) => {
    if (request.url().endsWith('/api/ai/chat/stream') && request.method() === 'POST') streamRequests += 1;
  });
  await page.type('#prompt-input', 'Dame otro ejemplo de sincronización');
  await page.click('#send-btn');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 4);
  await page.waitForFunction(() => !document.querySelector('#send-btn').disabled);
  const messages = await page.$$eval('[data-chat-turn] .nx-message-text', (els) => els.map((el) => el.textContent));
  assert.equal(messages[2], 'Dame otro ejemplo de sincronización');
  assert.match(messages[3], /Respuesta ficticia/);
  assert.equal(streamRequests, 1);
  assert.match(await page.$eval('[data-context]', (el) => el.textContent), /4 mensajes/);
});

await interaction('chat principal: acciones especializadas', '/dashboard/ai', async (page) => {
  let summaries = 0;
  page.on('request', (request) => {
    if (request.url().endsWith('/api/ai/summary') && request.method() === 'POST') summaries += 1;
  });
  await page.click('#btn-summarize');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 3);
  assert.match(await page.$eval('[data-chat-turn="assistant"]:last-child .nx-message-text', (el) => el.textContent), /semáforos protegen/);
  assert.equal(summaries, 1);
  assert.equal(await page.$eval('#btn-summarize', (el) => el.disabled), false);
});

await interaction('historial de conversaciones', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  await page.click('.conversation-item');
  await page.waitForFunction(() => document.querySelector('#chat-messages').innerText.includes('semáforo'));
  await page.click('#toggle-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), false);
});

await interaction('envío de chat conserva el historial', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  await page.click('.conversation-item');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 2);
  await page.type('#prompt-input', 'Dame otro ejemplo');
  await page.click('#send-btn');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 4);
  const messages = await page.$$eval('[data-chat-turn] .nx-message-text', (els) => els.map((el) => el.textContent));
  assert.equal(messages[0].includes('semáforo'), true);
  assert.equal(messages[3].includes('Respuesta ficticia'), true);
  assert.equal(await page.$$eval('.nx-chat-loading', (els) => els.length), 0);
});

await interaction('acciones de una respuesta', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  await page.click('.conversation-item');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 2);
  await page.click('[data-chat-turn="assistant"] .nx-message-actions button:first-child');
  await page.waitForFunction(() => document.querySelector('[data-chat-turn="assistant"] .nx-message-actions button:first-child').textContent.includes('Copiado'));
  await page.click('[data-chat-turn="assistant"] .nx-message-actions button:last-child');
  await page.waitForFunction(() => document.querySelectorAll('[data-chat-turn]').length === 3);
  const quiz = await page.$eval('[data-chat-turn="assistant"]:last-of-type .nx-message-text', (el) => el.textContent);
  assert.equal(quiz.includes('Cuestionario'), true);
  assert.equal(quiz.includes('¿Qué problema resuelve un semáforo?'), true);
});

await interaction('notas: modos e importación segura (modal)', '/dashboard/notes', async (page) => {
  await page.waitForFunction(() => document.getElementById('note-editor') !== null);
  const paneles = await page.$$eval('.nx-pane-label', (labels) => labels.length);
  assert.equal(paneles, 2);
  await page.click('#mode-preview');
  assert.equal(await page.$eval('#note-editor-pane', (el) => getComputedStyle(el).display), 'none');
  assert.equal(await page.$eval('#mode-preview', (el) => el.getAttribute('aria-pressed')), 'true');
  await page.click('#mode-split');
  assert.notEqual(await page.$eval('#note-editor-pane', (el) => getComputedStyle(el).display), 'none');
  // Importación segura: el botón abre SOLO el modal; el selector de carpetas
  // queda detrás de "Examinar" y la confirmación arranca deshabilitada.
  await page.click('#btn-import-vault');
  assert.equal(await page.$eval('#import-vault-modal', (el) => el.classList.contains('hidden')), false);
  assert.equal(await page.$eval('#confirm-import-btn', (el) => el.disabled), true);
  assert.equal(await page.$eval('input[name="conflict-rule"]:checked', (el) => el.value), 'keep-both');
  const importInput = await page.$('input[type="file"][webkitdirectory][hidden]');
  assert.ok(importInput, 'el selector de carpeta vive en el modal, no se lanza directo');
  assert.equal(await importInput.evaluate((el) => el.accept), '.md,.markdown');
  await page.keyboard.press('Escape');
  assert.equal(await page.$eval('#import-vault-modal', (el) => el.classList.contains('hidden')), true);
});

await interaction('MCP: secreto, ping y playground', '/dashboard/mcp', async (page) => {
  await page.click('#toggle-key-visibility');
  assert.equal(await page.$eval('#openrouter-key', (el) => el.type), 'text');
  await page.click('#btn-test-ping');
  await page.waitForFunction(() => /ms/.test(document.querySelector('#header-latency').textContent));
  await page.type('#chat-input', 'Prueba local');
  await page.click('#btn-send-message');
  await page.waitForFunction(() => document.querySelector('#chat-messages').innerText.includes('Mensaje recibido'));
});

await interaction('oficina: navegación y archivos recientes', '/office', async (page) => {
  assert.equal(await page.$$eval('.gen-office-tabs a', (links) => links.length), 5);
  assert.equal(await page.$$eval('.gen-office-launch-grid > a', (links) => links.length), 4);
  assert.equal(await page.$$eval('.gen-office-recent .gen-office-file-card', (cards) => cards.length), 4);
  const destinations = await page.$$eval('.gen-office-launch-grid > a', (links) => links.map((link) => link.getAttribute('href')));
  assert.deepEqual(destinations, ['/office/docs/new', '/office/sheets/new', '/office/presentations/new', '/office/library']);
  assert.equal(await page.$eval('#office-home-search', (input) => input.form.action.endsWith('/office/library/search')), true);
  await page.goto(`${base}/office/docs`, { waitUntil: 'networkidle0' });
  assert.equal(await page.$$eval('form[hx-delete="/office/docs/demo"]', (forms) => forms.length), 2);
  assert.equal(await page.$$eval('.gen-office-file-card a[href*="/export?fmt="]', (links) => links.length), 4);
  await page.goto(`${base}/office/sheets`, { waitUntil: 'networkidle0' });
  assert.equal(await page.$$eval('form[hx-post="/office/sheets/demo/delete"]', (forms) => forms.length), 1);
  assert.equal(await page.$$eval('.gen-office-file-card a[href="/office/sheets/demo/export"]', (links) => links.length), 1);
  await page.goto(`${base}/office/presentations`, { waitUntil: 'networkidle0' });
  assert.equal(await page.$$eval('form[hx-delete="/office/presentations/demo"]', (forms) => forms.length), 1);
  assert.equal(await page.$$eval('.gen-office-file-card a[href="/office/presentations/demo/present"]', (links) => links.length), 1);
});

await interaction('oficina: panel de herramientas adaptable', '/office/docs/demo', async (page) => {
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-hidden')), false);
  await page.click('[data-office-ai-toggle]');
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-hidden')), true);
  assert.equal(await page.$eval('[data-office-ai-toggle]', (el) => el.getAttribute('aria-expanded')), 'false');
  await page.click('[data-office-ai-toggle]');
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-hidden')), false);
});

await interaction('cuadrícula, fórmulas, inserción y CSV', '/office/sheets/demo', async (page) => {
  assert.equal(await page.$$eval('#grid td[contenteditable]', (cells) => cells.length), 25);
  await page.focus('#grid td[data-cell="B2"]');
  assert.equal(await page.$eval('#active-cell', (el) => el.textContent), 'B2');
  assert.equal(await page.$eval('#formula-input', (el) => el.value), '2');
  await page.$eval('#formula-input', (el) => { el.value = '=SUM(C2:C3)'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  assert.equal(await page.$eval('#grid td[data-cell="B2"]', (el) => el.textContent), '=SUM(C2:C3)');
  await page.evaluate(() => addOfficeGridRow());
  assert.equal(await page.$$eval('#grid tbody tr', (rows) => rows.length), 6);
  await page.evaluate(() => addOfficeGridColumn());
  assert.equal(await page.$$eval('#grid td[contenteditable]', (cells) => cells.length), 36);
  assert.equal(await page.$$eval('#grid thead th', (cells) => cells.length), 7);
  const csv = await page.evaluate(() => {
    const cell = document.querySelector('#grid td[data-cell="A1"]');
    cell.textContent = 'Actualizado, "NOVA"';
    serializeGrid();
    return document.querySelector('#csv_data').value;
  });
  assert.equal(csv.startsWith('"Actualizado, ""NOVA""",'), true);
  assert.match(await page.$eval('#sheet-grid-count', (el) => el.textContent), /6 filas · 6 columnas/);
});

await interaction('toolbar Markdown y error de guardado', '/office/docs/demo', async (page) => {
  await page.evaluate(() => {
    const editor = document.querySelector('#editor');
    editor.value = 'semáforo';
    editor.setSelectionRange(0, editor.value.length);
  });
  await page.click('.gen-office-doc-toolbar button');
  assert.equal(await page.$eval('#editor', (el) => el.value), '**semáforo**');
  assert.equal(await page.$eval('.gen-office-doc-form', (el) => el.getAttribute('hx-put')), '/office/docs/demo');
  await page.setRequestInterception(true);
  page.on('request', (request) => {
    if (request.method() === 'PUT' && request.url().endsWith('/office/docs/demo')) {
      request.respond({ status: 500, contentType: 'text/html', body: 'error simulado' });
    } else {
      request.continue();
    }
  });
  await page.click('#save-document-button');
  await page.waitForSelector('.sp-toast.err', { visible: true });
  assert.match(await page.$eval('.sp-toast.err', (toast) => toast.textContent), /No se pudo completar/);
}, viewports[0], [500]);

await interaction('selección y edición de diapositivas', '/office/presentations/demo', async (page) => {
  assert.equal(await page.$$eval('.gen-office-thumb', (cards) => cards.length), 5);
  assert.equal(await page.$eval('#new-slide-content', (el) => el.value.includes('<footer')), false);
  assert.equal(await page.$eval('[data-slide-panel="0"] .gen-office-slide-canvas', (el) => el.getBoundingClientRect().width > 500), true);
  assert.equal(await page.$$eval('.gen-office-slide-editor:not([hidden])', (panels) => panels.length), 1);
  await page.click('[data-slide-target="1"]');
  assert.equal(await page.$eval('[data-slide-panel="1"]', (el) => el.hidden), false);
  assert.equal(await page.$eval('[data-slide-panel="0"]', (el) => el.hidden), true);
  assert.equal(await page.$eval('#slide-position', (el) => el.textContent), '2 / 5');
  await page.$eval('#slide-title-1', (el) => { el.value = 'Semáforos en Rust'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  assert.equal(await page.$eval('#slide-preview-1 h2', (el) => el.textContent), 'Semáforos en Rust');
  await page.evaluate(() => openNewSlide());
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-hidden')), false);
});

await interaction('orden de diapositivas', '/office/presentations/demo', async (page) => {
  const order = await page.evaluate(() => { buildOrder(); return document.querySelector('#order').value; });
  assert.equal(order, '0,1,2,3,4');
  await page.setRequestInterception(true);
  const reorderRequest = page.waitForRequest((request) => request.url().endsWith('/office/presentations/demo/slides/reorder') && request.method() === 'POST');
  await page.click('#slide-0 button[aria-label="Mover diapositiva hacia abajo"]');
  const request = await reorderRequest;
  assert.equal(request.postData(), 'order=1%2C0%2C2%2C3%2C4');
  await request.respond({ status: 200, contentType: 'text/plain', body: 'ok' });
  await page.setRequestInterception(false);
});

await interaction('modo Presentar: botones y teclado', '/office/presentations/demo/present', async (page) => {
  assert.equal(await page.$$eval('[data-present-slide]:not([hidden])', (slides) => slides.length), 1);
  assert.equal(await page.$eval('#presentation-prev', (el) => el.disabled), true);
  await page.click('#presentation-next');
  assert.equal(await page.$eval('#presentation-position', (el) => el.textContent), '2 / 5');
  assert.equal(await page.$eval('[data-present-slide="1"]', (el) => el.hidden), false);
  await page.keyboard.press('End');
  assert.equal(await page.$eval('#presentation-position', (el) => el.textContent), '5 / 5');
  assert.equal(await page.$eval('#presentation-next', (el) => el.disabled), true);
  await page.keyboard.press('Home');
  assert.equal(await page.$eval('#presentation-position', (el) => el.textContent), '1 / 5');
});

await interaction('biblioteca: apertura, descarga e importación', '/office/library', async (page) => {
  const openHrefs = await page.$$eval('.gen-office-library-file', (links) => links.map((link) => link.getAttribute('href')));
  assert.deepEqual(openHrefs, ['/office/docs/demo', '/office/sheets/demo', '/office/docs/demo', '/office/presentations/demo']);
  assert.equal(await page.$$eval('.gen-office-library-actions a[download], .gen-office-library-actions a[href$="/download"]', (links) => links.length), 4);
  await page.select('#library-import-kind', 'sheets');
  assert.equal(await page.$eval('#library-import-form', (el) => el.getAttribute('hx-post')), '/office/library/import/sheets');
  await page.select('#library-import-kind', 'presentations');
  assert.equal(await page.$eval('#library-import-form', (el) => el.getAttribute('hx-post')), '/office/library/import/presentations');
});

await interaction('oficina móvil: cajón de herramientas', '/office/docs/demo', async (page) => {
  assert.equal(await page.$eval('#gen-office-ai-panel', (el) => getComputedStyle(el).display), 'none');
  await page.click('[data-office-ai-toggle]');
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-open')), true);
  assert.equal(await page.$eval('#gen-office-ai-panel', (el) => getComputedStyle(el).display), 'flex');
  assert.equal(await page.$eval('[data-office-ai-toggle]', (el) => el.getAttribute('aria-expanded')), 'true');
  await page.keyboard.press('Escape');
  assert.equal(await page.$eval('.gen-office-workspace', (el) => el.classList.contains('ai-open')), false);
}, viewports[1]);

await interaction('parcial HTMX del calendario', '/dashboard/calendar', async (page) => {
  await page.evaluate(() => window.htmx.ajax('GET', '/dashboard/calendar?partial=grid', { target: '#calendar-grid', swap: 'outerHTML' }));
  await page.waitForFunction(() => document.querySelector('#calendar-grid')?.innerText.includes('Mes actualizado'));
  assert.equal(await page.$$eval('#calendar-grid', (els) => els.length), 1);
  assert.equal(await page.$$eval('.nova-page .nova-page', (els) => els.length), 0);
  assert.equal(await page.$$eval('.nx-page .nx-page', (els) => els.length), 0);
});

await interaction('acordeones de vida escolar', '/dashboard/vida-escolar', async (page) => {
  const second = await page.$$('#lifeCards');
  assert.equal(await page.$$eval('.nova-life-card', (cards) => cards.length), 6);
  await page.click('.nova-life-card:nth-child(2) summary');
  assert.equal(await page.$eval('.nova-life-card:nth-child(2)', (el) => el.open), true);
});

await interaction('navegación semanal separada', '/dashboard/planner', async (page) => {
  const nav = await page.evaluate(() => {
    const links = [...document.querySelectorAll('.nova-planner-nav a')].map((el) => el.getBoundingClientRect());
    const label = document.querySelector('.nova-planner-nav strong').getBoundingClientRect();
    return { previousRight: links[0].right, nextLeft: links[1].left, labelLeft: label.left, labelRight: label.right };
  });
  assert.equal(nav.previousRight <= nav.labelLeft, true);
  assert.equal(nav.labelRight <= nav.nextLeft, true);
}, viewports[1]);

await interaction('horario personal: materias, bloques y procedencia', '/dashboard/horario', async (page) => {
  const text = await page.$eval('.nx-schedule', (el) => el.textContent);
  for (const expected of ['Unidad Educativa Álamos', '2026–2027', 'Santiago Pasquel', '07:35 - 08:15 — Religión', '09:15 - 09:55 — Tutoría', '10:35 - 11:05 — Recreo', '12:25 - 13:05 — Plan lector', '13:05 - 13:45 — Matemática']) {
    assert(text.includes(expected), `Falta en el horario: ${expected}`);
  }
  assert.equal(await page.$$('.nova-schedule-day li').then((items) => items.length), 50);
  assert.equal(await page.$$eval('.nova-schedule-day', (days) => days.slice(5).filter((day) => day.textContent.includes('Sin clases')).length), 2);
  assert.equal(await page.$eval('.nx-schedule-source[href*="mikareno"]', (el) => el.getAttribute('href')), '/dashboard/horario?fuente=mikareno');
});

await interaction('chat móvil: cajón, búsqueda y sugerencias', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  const composerVisible = await page.evaluate(() => document.querySelector('.nx-composer').getBoundingClientRect().bottom <= innerHeight + 1);
  assert.equal(composerVisible, true);
  await page.click('#toggle-history-btn');
  assert.equal(await page.$eval('.nx-chat', (el) => el.classList.contains('is-history-open')), true);
  const bounds = await page.evaluate(() => {
    const second = document.querySelectorAll('.conversation-item')[1].getBoundingClientRect();
    const quota = document.querySelector('.nx-chat-quota').getBoundingClientRect();
    return { secondBottom: second.bottom, quotaTop: quota.top };
  });
  assert.equal(bounds.secondBottom <= bounds.quotaTop, true);
  await page.type('#session-search', 'redes');
  await page.waitForFunction(() => [...document.querySelectorAll('.conversation-item')].filter((el) => !el.hidden).length === 1);
  assert.equal(await page.$eval('.conversation-item:not([hidden])', (el) => el.textContent.includes('redes')), true);
  await page.click('#close-history-btn');
  assert.equal(await page.$eval('.nx-chat', (el) => el.classList.contains('is-history-open')), false);
  await settle(page, 260);
  await page.click('[data-chat-prompt]');
  await page.waitForFunction(() => document.querySelector('#prompt-input').value.startsWith('Explícame'), { polling: 'raf', timeout: 2500 });
  assert.equal(await page.$eval('#prompt-input', (el) => el.value.startsWith('Explícame')), true);
}, viewports[1]);

await interaction('título de nota en móvil', '/dashboard/notes', async (page) => {
  await page.waitForFunction(() => document.querySelector('.nx-note-title input')?.value.includes('Concurrencia'));
  const bounds = await page.evaluate(() => {
    const row = document.querySelector('.nx-note-title');
    const input = row.querySelector('input').getBoundingClientRect();
    const status = row.lastElementChild.getBoundingClientRect();
    return { inputWidth: input.width, inputBottom: input.bottom, statusTop: status.top };
  });
  assert.equal(bounds.inputWidth > 240, true);
  assert.equal(bounds.statusTop >= bounds.inputBottom, true);
  await page.click('#mode-preview');
  assert(await page.$eval('#note-preview', (el) => getComputedStyle(el).display !== 'none'), 'Lectura no muestra la vista previa en móvil');
  assert(await page.$eval('#note-editor-pane', (el) => getComputedStyle(el).display === 'none'), 'Lectura móvil sigue mostrando Markdown');
  await page.click('#mode-split');
  assert(await page.$eval('#note-preview', (el) => getComputedStyle(el).display !== 'none'), 'Dividido no recupera la vista previa en móvil');
}, viewports[1]);

await interaction('archivos móviles de biblioteca', '/office/library', async (page) => {
  const layout = await page.evaluate(() => {
    const browser = document.querySelector('.gen-office-library-browser').getBoundingClientRect();
    const rows = [...document.querySelectorAll('.gen-office-library-table tbody tr')].map((el) => el.getBoundingClientRect());
    return {
      documentWidth: document.documentElement.scrollWidth,
      rowCount: rows.length,
      actions: document.querySelectorAll('.gen-office-library-actions .gen-office-icon-button').length,
      rowsInside: rows.every((row) => row.left >= browser.left && row.right <= browser.right),
      labels: document.querySelectorAll('.gen-office-library-table td[data-label]').length,
    };
  });
  assert.equal(layout.documentWidth, 390);
  assert.equal(layout.rowCount, 4);
  assert.equal(layout.actions, 12);
  assert.equal(layout.rowsInside, true);
  assert.equal(layout.labels, 16);
}, viewports[1]);

fs.writeFileSync('/tmp/nova-full-audit.json', JSON.stringify({ generatedAt: new Date().toISOString(), results, failures }, null, 2));
await browser.close();
console.log(`\nNova full audit: ${results.length} comprobaciones · ${failures.length} fallos`);
if (failures.length) {
  console.error(JSON.stringify(failures, null, 2));
  process.exitCode = 1;
} else {
  console.log('Nova full audit: OK');
}
