/**
 * Coalesce concurrent CollectNetworkInfo calls (S10).
 *
 * Keys are **instance UUIDs** (shared by status page + NetworkList VIP fetch).
 * - Single-key callers (status): coalesce watch/poll onto one promise.
 * - Multi-key callers (NetworkList batch): register every id under the same
 *   promise so status `isAny` / single-flight can see the tunnel is busy.
 * - If status hits a multi-key (batch) flight, it waits for release then runs
 *   its own typed collect (batch returns a Record, not a single info).
 *
 * Important:
 * 1. Claim keys **before** invoking `factory()` (async factories yield at first
 *    await; late registration would miss concurrent callers).
 * 2. Clear keys **before** resolving/rejecting the shared promise so a sequential
 *    caller after `await` does not coalesce onto an already-settled flight.
 */

const inFlight = new Map<string, Promise<unknown>>()

function keysHolding(promise: Promise<unknown>): string[] {
    const out: string[] = []
    for (const [k, p] of inFlight) {
        if (p === promise) {
            out.push(k)
        }
    }
    return out
}

function waitForFlights(waiters: Promise<unknown>[]): Promise<void> {
    if (waiters.length === 0) {
        return Promise.resolve()
    }
    return Promise.allSettled(waiters).then(() => undefined)
}

function flightsForKeys(keys: string[]): Promise<unknown>[] {
    return [
        ...new Set(
            keys
                .map((k) => inFlight.get(k))
                .filter((p): p is Promise<unknown> => p != null),
        ),
    ]
}

function clearKeysIfPending(keys: string[], pending: Promise<unknown>) {
    for (const k of keys) {
        if (inFlight.get(k) === pending) {
            inFlight.delete(k)
        }
    }
}

/** True when any of the instance ids already has an in-flight collect. */
export function isAnyCollectNetworkInfoInFlight(keys: string[]): boolean {
    return keys.some((k) => inFlight.has(k))
}

/**
 * Run `factory` once for all `keys` (instance ids); concurrent overlapping
 * callers wait for release then retry.
 */
export function collectNetworkInfoSingleFlightForKeys<T>(
    keys: string[],
    factory: () => Promise<T>,
): Promise<T> {
    const unique = [...new Set(keys.filter(Boolean))]
    if (unique.length === 0) {
        return factory()
    }
    if (unique.length === 1) {
        return collectNetworkInfoSingleFlight(unique[0]!, factory)
    }

    if (isAnyCollectNetworkInfoInFlight(unique)) {
        return waitForFlights(flightsForKeys(unique)).then(() =>
            collectNetworkInfoSingleFlightForKeys(unique, factory),
        )
    }

    let resolve!: (value: T) => void
    let reject!: (reason: unknown) => void
    const pending = new Promise<T>((res, rej) => {
        resolve = res
        reject = rej
    })

    for (const k of unique) {
        inFlight.set(k, pending)
    }

    factory().then(
        (value) => {
            clearKeysIfPending(unique, pending)
            resolve(value)
        },
        (reason) => {
            clearKeysIfPending(unique, pending)
            reject(reason)
        },
    )
    return pending
}

/** Run `factory` once per instance id; concurrent callers await the same promise. */
export function collectNetworkInfoSingleFlight<T>(
    key: string,
    factory: () => Promise<T>,
): Promise<T> {
    const existing = inFlight.get(key)
    if (existing) {
        const holders = keysHolding(existing)
        // True coalesce: only this instance id holds the promise (status watch+poll).
        if (holders.length === 1 && holders[0] === key) {
            return existing as Promise<T>
        }
        // Batch VIP (or other multi-key) owns this id — wrong result shape; wait then fetch.
        return existing.then(
            () => collectNetworkInfoSingleFlight(key, factory),
            () => collectNetworkInfoSingleFlight(key, factory),
        )
    }

    let resolve!: (value: T) => void
    let reject!: (reason: unknown) => void
    const pending = new Promise<T>((res, rej) => {
        resolve = res
        reject = rej
    })

    inFlight.set(key, pending)

    factory().then(
        (value) => {
            clearKeysIfPending([key], pending)
            resolve(value)
        },
        (reason) => {
            clearKeysIfPending([key], pending)
            reject(reason)
        },
    )
    return pending
}
