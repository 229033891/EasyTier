import { describe, expect, it, vi } from 'vitest'
import {
    collectNetworkInfoSingleFlight,
    collectNetworkInfoSingleFlightForKeys,
    isAnyCollectNetworkInfoInFlight,
} from '../src/modules/collectNetworkInfoSingleFlight'

describe('collectNetworkInfoSingleFlight', () => {
    it('coalesces concurrent callers onto one factory run', async () => {
        let runs = 0
        let release!: (v: string) => void
        const gate = new Promise<string>((resolve) => {
            release = resolve
        })
        const factory = vi.fn(async () => {
            runs += 1
            return gate
        })

        const p1 = collectNetworkInfoSingleFlight('inst-a', factory)
        const p2 = collectNetworkInfoSingleFlight('inst-a', factory)
        expect(isAnyCollectNetworkInfoInFlight(['inst-a'])).toBe(true)
        expect(isAnyCollectNetworkInfoInFlight(['inst-a', 'inst-b'])).toBe(true)
        expect(factory).toHaveBeenCalledTimes(1)

        release('ok')
        await expect(Promise.all([p1, p2])).resolves.toEqual(['ok', 'ok'])
        expect(runs).toBe(1)
        expect(isAnyCollectNetworkInfoInFlight(['inst-a'])).toBe(false)
    })

    it('allows a new flight after the previous settles', async () => {
        const factory = vi.fn(async () => 'done')
        await collectNetworkInfoSingleFlight('inst-b', factory)
        await collectNetworkInfoSingleFlight('inst-b', factory)
        expect(factory).toHaveBeenCalledTimes(2)
    })

    // NetworkList fetchMetasForDevices 契约：批量 ids 里任一实例与状态页
    // 的单实例 collect 共享 key，in-flight 即整批跳过本页 VIP 拉取（S10）。
    it('any-of check marks the whole batch as blocked while one id is in flight', async () => {
        let release!: (v: string) => void
        const gate = new Promise<string>((resolve) => {
            release = resolve
        })
        const p = collectNetworkInfoSingleFlight('inst-c', async () => gate)
        expect(isAnyCollectNetworkInfoInFlight(['inst-c', 'inst-d'])).toBe(true)

        release('ok')
        await p
        expect(isAnyCollectNetworkInfoInFlight(['inst-c', 'inst-d'])).toBe(false)
    })

    it('forKeys registers every instance id so status can see list-side batch', async () => {
        let release!: (v: Record<string, string>) => void
        const gate = new Promise<Record<string, string>>((resolve) => {
            release = resolve
        })
        const batch = collectNetworkInfoSingleFlightForKeys(['inst-x', 'inst-y'], async () => gate)

        expect(isAnyCollectNetworkInfoInFlight(['inst-x'])).toBe(true)
        expect(isAnyCollectNetworkInfoInFlight(['inst-y'])).toBe(true)

        // Status hits a multi-key flight: waits, then runs its own typed collect.
        const statusFactory = vi.fn(async () => 'status-ok')
        const statusPromise = collectNetworkInfoSingleFlight('inst-x', statusFactory)
        expect(statusFactory).not.toHaveBeenCalled()

        release({ 'inst-x': 'vip', 'inst-y': 'vip' })
        await expect(batch).resolves.toEqual({ 'inst-x': 'vip', 'inst-y': 'vip' })
        await expect(statusPromise).resolves.toBe('status-ok')
        expect(statusFactory).toHaveBeenCalledTimes(1)
        expect(isAnyCollectNetworkInfoInFlight(['inst-x', 'inst-y'])).toBe(false)
    })

    it('claims keys before factory awaits so concurrent callers see in-flight', async () => {
        let release!: (v: string) => void
        const gate = new Promise<string>((resolve) => {
            release = resolve
        })
        let sawInFlightInsideFactory = false
        const p = collectNetworkInfoSingleFlight('inst-claim', async () => {
            // First await of an async factory — key must already be claimed.
            sawInFlightInsideFactory = isAnyCollectNetworkInfoInFlight(['inst-claim'])
            return gate
        })
        expect(sawInFlightInsideFactory).toBe(true)
        expect(isAnyCollectNetworkInfoInFlight(['inst-claim'])).toBe(true)
        release('ok')
        await expect(p).resolves.toBe('ok')
    })
})
