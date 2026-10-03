// Browser preference compatibility for the upstream brand. New values always
// win. Promotion is best-effort and retains old keys for older clients.
// A full quota must not hide readable old state.
// Keep the legacy name here literal even when changing the current brand.
function legacyKey(key: string): string | null {
  if (key.startsWith("yaran:")) return "dux:" + key.slice(6)
  if (key.startsWith("yaran-")) return "dux-" + key.slice(6)
  return null
}

export function readBrandedStorage(store: Storage, key: string): string | null {
  const current = store.getItem(key)
  if (current !== null) return current
  const old = legacyKey(key)
  if (!old) return null
  const value = store.getItem(old)
  if (value !== null) {
    try {
      store.setItem(key, value)
    } catch {
      // Return the old preference even if promotion cannot be persisted.
    }
  }
  return value
}

export function removeBrandedStorage(store: Storage, key: string): void {
  const old = legacyKey(key)
  // Retire the fallback first, so resetting cannot resurrect upstream state.
  if (old) store.removeItem(old)
  store.removeItem(key)
}
