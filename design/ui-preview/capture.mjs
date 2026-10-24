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
const routes = [...createPages('dashboard').keys()].filter((route) => route !== '/' && (!onlyRoutes.size || onlyRoutes.has(route)));
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
  '/office/presentations/demo/present', '/office/library',
]);
const results = [];
const failures = [];

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
    const metrics = await page.evaluate(() => {
      const ids = [...document.querySelectorAll('[id]')].map((el) => el.id);
      const main = document.querySelector('#contenido');
      const nova = document.querySelector('.nova-page, .nova-dashboard');
      const overflowing = [...document.querySelectorAll('#contenido *')].map((el) => {
        const rect = el.getBoundingClientRect();
        return { tag: el.tagName, className: String(el.className).slice(0, 90), left: Math.round(rect.left), right: Math.round(rect.right) };
      }).filter((item) => item.right > innerWidth + 2 || item.left < -2).slice(0, 8);
      return {
        documentWidth: document.documentElement.scrollWidth,
        viewportWidth: innerWidth,
        navItems: document.querySelectorAll('#app-rail [data-path]').length,
        menuVisibility: getComputedStyle(document.querySelector('#app-rail')).visibility,
        duplicateIds: ids.filter((id, index) => ids.indexOf(id) !== index),
        slotsLeft: document.documentElement.innerHTML.includes('<!--SLOT:'),
        mainLength: main ? main.innerText.trim().length : 0,
        hasNova: Boolean(nova),
        htmxLoaded: typeof window.htmx === 'function' || typeof window.htmx === 'object',
        versionVisible: document.body.textContent.includes('3.0'),
        overflowing,
      };
    });
    const record = { viewport: viewport.name, route, ...metrics, errors, badResponses, external };
    results.push(record);
    const problems = [];
    if (metrics.documentWidth !== metrics.viewportWidth) problems.push(`overflow ${metrics.documentWidth}/${metrics.viewportWidth}`);
    if (metrics.navItems !== 22) problems.push(`nav=${metrics.navItems}`);
    if (metrics.menuVisibility !== 'hidden') problems.push(`menú=${metrics.menuVisibility}`);
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

async function interaction(name, route, callback, viewport = viewports[0]) {
  if (process.env.NOVA_INTERACTIONS === '0') return;
  const { page, errors, badResponses, external } = await openPage(route, viewport, false);
  try {
    await callback(page);
    await settle(page);
    assert.deepEqual(errors, [], `Consola limpia en ${name}`);
    assert.deepEqual(badResponses, [], `Sin respuestas HTTP erróneas en ${name}`);
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

await interaction('shell y menús', '/dashboard', async (page) => {
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'visible');
  assert.equal(await page.$eval('#rail-toggle', (el) => el.getAttribute('aria-expanded')), 'true');
  assert.equal(await page.$eval('body', (el) => el.style.overflow), 'hidden');
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => getComputedStyle(document.querySelector('#app-rail')).visibility === 'hidden', { polling: 'raf', timeout: 2500 });
  assert.equal(await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility), 'hidden');
  await page.click('#toggle-create-menu');
  assert.equal(await page.$eval('#create-modal-popover', (el) => el.classList.contains('hidden')), false);
  await page.click('#close-create-menu');
  await page.click('#quick-create-btn');
  assert.equal(await page.$eval('#quick-task-modal', (el) => el.classList.contains('hidden')), false);
  await page.click('#cancel-modal-btn');
  await page.click('#topProfile');
  assert.equal(await page.$eval('#user-menu', (el) => el.hidden), false);
  await page.keyboard.press('Escape');
});

await interaction('cajón móvil', '/dashboard', async (page) => {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
  await page.click('#rail-toggle');
  await settle(page);
  assert.equal(await page.$eval('body', (el) => el.classList.contains('rail-open')), true);
  const vaultBelow = await page.evaluate(() => document.querySelector('.nova-menu .app-vault').getBoundingClientRect().top >= document.querySelector('.nova-menu .app-system-nav').getBoundingClientRect().bottom - 1);
  assert.equal(vaultBelow, true);
}, viewports[1]);

await interaction('fuentes del asistente', '/dashboard/ai', async (page) => {
  await page.click('#toggle-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), true);
  await page.click('#toggle-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), false);
});

await interaction('historial de conversaciones', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  await page.click('.conversation-item');
  await page.waitForFunction(() => document.querySelector('#chat-messages').innerText.includes('semáforo'));
  await page.click('#toggle-sources-btn');
  assert.equal(await page.$eval('#sources-sidebar', (el) => el.classList.contains('hidden')), false);
});

await interaction('notas: hidratación, modos y modal', '/dashboard/notes', async (page) => {
  await page.waitForFunction(() => document.querySelector('#note-editor')?.value.includes('semáforo'));
  await page.click('#mode-preview');
  assert.equal(await page.$eval('#note-editor-pane', (el) => getComputedStyle(el).display), 'none');
  assert.equal(await page.$eval('#mode-preview', (el) => el.getAttribute('aria-pressed')), 'true');
  await page.click('#mode-split');
  assert.notEqual(await page.$eval('#note-editor-pane', (el) => getComputedStyle(el).display), 'none');
  await page.click('#btn-import-vault');
  assert.equal(await page.$eval('#import-vault-modal', (el) => el.classList.contains('hidden')), false);
  await page.click('#cancel-modal-btn');
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

await interaction('cuadrícula editable y CSV', '/office/sheets/demo', async (page) => {
  assert.equal(await page.$$eval('#grid td[contenteditable]', (cells) => cells.length), 25);
  const csv = await page.evaluate(() => {
    const cell = document.querySelector('#grid td[contenteditable]');
    cell.textContent = 'Actualizado';
    serializeGrid();
    return document.querySelector('#csv_data').value;
  });
  assert.equal(csv.startsWith('Actualizado,'), true);
});

await interaction('toolbar Markdown', '/office/docs/demo', async (page) => {
  await page.evaluate(() => {
    const editor = document.querySelector('#editor');
    editor.value = 'semáforo';
    editor.setSelectionRange(0, editor.value.length);
  });
  await page.click('.nova-markdown-toolbar button');
  assert.equal(await page.$eval('#editor', (el) => el.value), '**semáforo**');
});

await interaction('orden de diapositivas', '/office/presentations/demo', async (page) => {
  const order = await page.evaluate(() => { buildOrder(); return document.querySelector('#order').value; });
  assert.equal(order, '0,1,2');
});

await interaction('tipo de importación en biblioteca', '/office/library', async (page) => {
  await page.select('#library-import-kind', 'sheets');
  assert.equal(await page.$eval('#library-import-form', (el) => el.getAttribute('hx-post')), '/office/library/import/sheets');
});

await interaction('parcial HTMX del calendario', '/dashboard/calendar', async (page) => {
  await page.evaluate(() => window.htmx.ajax('GET', '/dashboard/calendar?partial=grid', { target: '#calendar-grid', swap: 'outerHTML' }));
  await page.waitForFunction(() => document.querySelector('#calendar-grid')?.innerText.includes('Mes actualizado'));
  assert.equal(await page.$$eval('#calendar-grid', (els) => els.length), 1);
  assert.equal(await page.$$eval('.nova-page .nova-page', (els) => els.length), 0);
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

await interaction('historial móvil sin solapamiento', '/ai', async (page) => {
  await page.waitForFunction(() => document.querySelectorAll('.conversation-item').length === 2);
  const bounds = await page.evaluate(() => {
    const second = document.querySelectorAll('.conversation-item')[1].getBoundingClientRect();
    const quota = document.querySelector('.nova-chat-quota').getBoundingClientRect();
    return { secondBottom: second.bottom, quotaTop: quota.top };
  });
  assert.equal(bounds.secondBottom <= bounds.quotaTop, true);
}, viewports[1]);

await interaction('título de nota en móvil', '/dashboard/notes', async (page) => {
  await page.waitForFunction(() => document.querySelector('.nova-note-title-row input')?.value.includes('Concurrencia'));
  const bounds = await page.evaluate(() => {
    const row = document.querySelector('.nova-note-title-row');
    const input = row.querySelector('input').getBoundingClientRect();
    const status = row.lastElementChild.getBoundingClientRect();
    return { inputWidth: input.width, inputBottom: input.bottom, statusTop: status.top };
  });
  assert.equal(bounds.inputWidth > 240, true);
  assert.equal(bounds.statusTop >= bounds.inputBottom, true);
}, viewports[1]);

await interaction('archivos móviles de biblioteca', '/office/library', async (page) => {
  const layout = await page.evaluate(() => {
    const browser = document.querySelector('.nova-library-browser').getBoundingClientRect();
    const rows = [...document.querySelectorAll('.nova-library-table tbody tr')].map((el) => el.getBoundingClientRect());
    return {
      documentWidth: document.documentElement.scrollWidth,
      rowCount: rows.length,
      actions: document.querySelectorAll('.nova-library-actions .nova-icon-button').length,
      rowsInside: rows.every((row) => row.left >= browser.left && row.right <= browser.right),
      labels: document.querySelectorAll('.nova-library-table td[data-label]').length,
    };
  });
  assert.equal(layout.documentWidth, 390);
  assert.equal(layout.rowCount, 3);
  assert.equal(layout.actions, 6);
  assert.equal(layout.rowsInside, true);
  assert.equal(layout.labels, 12);
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
