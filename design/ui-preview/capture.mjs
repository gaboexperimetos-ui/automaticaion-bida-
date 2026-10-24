import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import chromium from './node_modules/@sparticuz/chromium/build/esm/index.js';
import { inflate } from './node_modules/@sparticuz/chromium/build/esm/lambdafs.js';
import { setupLambdaEnvironment } from './node_modules/@sparticuz/chromium/build/esm/helper.js';

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
const page = await browser.newPage();
const errors = [];
page.on('pageerror', (error) => errors.push(error.message));
page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });

async function open(width, height) {
  await page.setViewport({ width, height, deviceScaleFactor: 1 });
  await page.goto('http://127.0.0.1:3000/dashboard', { waitUntil: 'networkidle0' });
  await page.evaluate(() => document.fonts.ready);
  await settle(700);
}
async function settle(ms = 350) {
  await new Promise((resolve) => setTimeout(resolve, ms));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

await open(1440, 1000);
const desktop = await page.evaluate(() => ({
  width: innerWidth,
  documentWidth: document.documentElement.scrollWidth,
  menuVisibility: getComputedStyle(document.querySelector('#app-rail')).visibility,
  contentPadding: getComputedStyle(document.querySelector('#app-rail-content')).paddingLeft,
  topbarLeft: Math.round(document.querySelector('#app-topbar').getBoundingClientRect().left),
  navItems: document.querySelectorAll('#app-rail [data-path]').length,
  panels: document.querySelectorAll('.nova-panel').length,
  slotsLeft: document.documentElement.innerHTML.includes('<!--SLOT:'),
  duplicateIds: [...document.querySelectorAll('[id]')].map((el) => el.id).filter((id, index, all) => all.indexOf(id) !== index),
}));
console.log('desktop', desktop);
await page.screenshot({ path: '/tmp/nova-desktop.png', fullPage: true });
await page.click('#rail-toggle');
await settle();
const desktopMenu = await page.evaluate(() => ({
  visibility: getComputedStyle(document.querySelector('#app-rail')).visibility,
  transform: getComputedStyle(document.querySelector('#app-rail')).transform,
  bodyOverflow: document.body.style.overflow,
  expanded: document.querySelector('#rail-toggle').getAttribute('aria-expanded'),
}));
console.log('desktop menu', desktopMenu);
await page.screenshot({ path: '/tmp/nova-desktop-menu.png', fullPage: false });
await page.keyboard.press('Escape');
await settle();
const desktopMenuClosed = await page.$eval('#app-rail', (el) => getComputedStyle(el).visibility);
console.log('desktop menu closed', desktopMenuClosed);
await page.click('#toggle-create-menu');
const createVisible = await page.$eval('#create-modal-popover', (el) => !el.classList.contains('hidden'));
console.log('create visible', createVisible);
await page.screenshot({ path: '/tmp/nova-create.png', fullPage: false });
await page.click('#close-create-menu');
await page.click('#quick-create-btn');
const modalVisible = await page.$eval('#quick-task-modal', (el) => !el.classList.contains('hidden'));
console.log('modal visible', modalVisible);
await page.screenshot({ path: '/tmp/nova-modal.png', fullPage: false });
await page.click('#cancel-modal-btn');
await page.click('#topProfile');
const accountVisible = await page.$eval('#user-menu', (el) => !el.hidden);
console.log('account visible', accountVisible);
await page.keyboard.press('Escape');

await open(390, 844);
const mobile = await page.evaluate(() => ({
  width: innerWidth,
  documentWidth: document.documentElement.scrollWidth,
  menuVisibility: getComputedStyle(document.querySelector('#app-rail')).visibility,
  topbarWidth: Math.round(document.querySelector('#app-topbar').getBoundingClientRect().width),
  overflowElements: [...document.querySelectorAll('body *')].map((el) => {
    const rect = el.getBoundingClientRect();
    return { tag: el.tagName, className: String(el.className).slice(0, 100), left: Math.round(rect.left), right: Math.round(rect.right), width: Math.round(rect.width) };
  }).filter((item) => item.right > innerWidth + 1 || item.left < -1).slice(0, 18),
  vaultPosition: getComputedStyle(document.querySelector('.nova-menu .app-vault')).position,
}));
console.log('mobile', mobile);
await page.screenshot({ path: '/tmp/nova-mobile.png', fullPage: true });
await page.click('#rail-toggle');
await settle();
const mobileMenu = await page.evaluate(() => ({
  open: document.body.classList.contains('rail-open'),
  visibility: getComputedStyle(document.querySelector('#app-rail')).visibility,
  transform: getComputedStyle(document.querySelector('#app-rail')).transform,
  vaultBelowSystem: document.querySelector('.nova-menu .app-vault').getBoundingClientRect().top >= document.querySelector('.nova-menu .app-system-nav').getBoundingClientRect().bottom - 1,
}));
console.log('mobile menu', mobileMenu);
await page.screenshot({ path: '/tmp/nova-mobile-menu.png', fullPage: false });
console.log('errors', errors);
await browser.close();

assert.equal(desktop.documentWidth, desktop.width, 'El documento de escritorio no debe desbordar');
assert.equal(desktop.menuVisibility, 'hidden', 'El menú debe iniciar cerrado en escritorio');
assert.equal(desktop.contentPadding, '0px', 'El contenido debe usar todo el ancho');
assert.equal(desktop.topbarLeft, 16, 'La cabecera debe flotar a 16 px');
assert.equal(desktop.navItems, 22, 'Deben conservarse los 22 destinos');
assert.equal(desktop.panels, 4, 'Deben existir los cuatro paneles principales de Nova');
assert.equal(desktop.slotsLeft, false, 'No debe quedar ningún slot sin renderizar');
assert.deepEqual(desktop.duplicateIds, [], 'No debe haber IDs duplicados');
assert.equal(desktopMenu.visibility, 'visible', 'El menú de escritorio debe abrirse');
assert.equal(desktopMenu.bodyOverflow, 'hidden', 'El body debe bloquearse con el menú abierto');
assert.equal(desktopMenu.expanded, 'true', 'El disparador debe exponer aria-expanded=true');
assert.equal(desktopMenuClosed, 'hidden', 'Escape debe cerrar el menú');
assert.equal(createVisible, true, 'El menú de creación debe abrirse');
assert.equal(modalVisible, true, 'El modal de tarea debe abrirse');
assert.equal(accountVisible, true, 'El menú de cuenta debe abrirse');
assert.equal(mobile.documentWidth, mobile.width, 'El documento móvil no debe desbordar');
assert.equal(mobile.menuVisibility, 'hidden', 'El menú debe iniciar cerrado en móvil');
assert.equal(mobile.vaultPosition, 'static', 'Vault debe seguir el flujo del menú móvil');
assert.equal(mobileMenu.open, true, 'El menú móvil debe abrirse');
assert.equal(mobileMenu.visibility, 'visible', 'El menú móvil debe ser visible');
assert.equal(mobileMenu.vaultBelowSystem, true, 'Vault no debe superponerse a los accesos');
assert.deepEqual(errors, [], 'La consola del navegador debe quedar limpia');
console.log('Nova audit: OK');
