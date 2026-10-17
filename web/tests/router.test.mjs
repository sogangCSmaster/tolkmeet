import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const router = loadModule('lib/router.ts', {
  react: { useEffect() {}, useRef() {}, useSyncExternalStore() {} },
  './auth': { useAuth() {} },
})

test('next accepts only same-origin paths', () => {
  for (const ok of ['/', '/account', '/account#data', '/checkout?item=pro', '/billing/done?item=hours&q=5'])
    assert.equal(router.safeNext(ok), ok)
  for (const bad of [null, '', 'account', '//evil.example', '/\\evil.example', '/x\\y', 'https://evil.example', 'javascript:alert(1)', '/\t/evil.example', '/\n/evil.example', '/a\u0085b'])
    assert.equal(router.safeNext(bad), null, JSON.stringify(bad))
})

test('next never points back to an auth page', () => {
  for (const bad of ['/login', '/login/', '/login?next=%2Faccount', '/signup#x', '/verify?email=a%40b.c', '/forgot', '/reset?token=x'])
    assert.equal(router.safeNext(bad), null, bad)
  for (const ok of ['/loginx', '/account/login', '/meetings?next=/login'])
    assert.equal(router.safeNext(ok), ok)
})
