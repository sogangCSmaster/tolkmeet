import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

// fetchMe가 돌려주는 모양: 로그인했으면 user, 아니면 null
const host = { user: { name: 'host' }, features: {} }
const guest = { user: null, features: {} }

function authStore(logout = async () => {}) {
  const requests = []
  const store = loadModule('lib/auth.ts', {
    react: { useSyncExternalStore: (_, snapshot) => snapshot() },
    './api': {
      fetchMe() {
        const request = deferred()
        requests.push(request)
        return request.promise
      },
      logout,
      onUnauthorized() {},
    },
  })
  return { store, requests }
}

test('logout HTTP failures reject instead of reporting success', async () => {
  const api = loadModule('lib/api.ts', {}, { fetch: async () => ({ ok: false, status: 500 }) })
  await assert.rejects(api.logout(), /status 500/)
})

test('failed logout keeps the authenticated UI state for retry', async () => {
  const { store, requests } = authStore(async () => { throw new Error('offline') })
  requests[0].resolve(host)
  await requests[0].promise
  assert.equal(store.useAuth().status, 'host')
  assert.equal(await store.logout(), false)
  assert.equal(store.useAuth().status, 'host')
})

test('refresh started before logout cannot restore a logged-out account', async () => {
  const { store, requests } = authStore()
  assert.equal(await store.logout(), true)
  requests[0].resolve(host)
  await requests[0].promise
  assert.equal(store.useAuth().status, 'guest')
})

test('refresh started during logout cannot restore a logged-out account', async () => {
  const response = deferred()
  const { store, requests } = authStore(() => response.promise)
  const logout = store.logout()
  const refresh = store.refreshAuth()
  response.resolve()
  assert.equal(await logout, true)
  requests[1].resolve(host)
  await refresh
  assert.equal(store.useAuth().status, 'guest')
})

test('a stale initial guest response cannot overwrite a newer login refresh', async () => {
  const { store, requests } = authStore()
  const latest = store.refreshAuth()
  requests[1].resolve(host)
  await latest
  requests[0].resolve(guest)
  await requests[0].promise
  assert.equal(store.useAuth().status, 'host')
})
