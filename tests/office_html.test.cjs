const test = require('node:test');
const assert = require('node:assert/strict');
const { previewDocument } = require('../static/app/office-html.js');

const policyStart = '<meta http-equiv="Content-Security-Policy"';

test('aplica CSP antes del HTML del usuario y conserva el documento', () => {
  const source = '<!doctype html><html lang="es"><head><title>A</title></head><body><h1>Hola</h1></body></html>';
  const html = previewDocument(source);
  assert.ok(html.startsWith('<!doctype html>' + policyStart));
  assert.match(html, /default-src 'none'/);
  assert.match(html, /script-src 'none'/);
  assert.match(html, /img-src data:/);
  assert.match(html, /connect-src 'none'/);
  assert.match(html, /form-action 'none'/);
  assert.ok(html.indexOf(policyStart) < html.indexOf('<html lang="es">'));
  assert.ok(html.endsWith(source));
});

test('la política se mantiene delante de fragmentos HTML sin cabecera', () => {
  const fragment = '<main><h1>Fragmento</h1></main>';
  const html = previewDocument(fragment);
  assert.ok(html.startsWith('<!doctype html>' + policyStart));
  assert.ok(html.endsWith(fragment));
});

test('un head falso dentro de un comentario no retrasa la CSP', () => {
  const source = '<!-- <head> -->\n<img src="https://example.invalid/pixel">';
  const html = previewDocument(source);
  assert.ok(html.indexOf(policyStart) < html.indexOf('<!-- <head> -->'));
  assert.ok(html.indexOf(policyStart) < html.indexOf('https://example.invalid/pixel'));
});
