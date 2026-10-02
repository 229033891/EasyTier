import { TOAST_LIFE, Utils } from 'easytier-frontend-lib';
import type { useToast } from 'primevue';
import type ApiClient from './api';

type Toast = ReturnType<typeof useToast>;

/** Last successful `/devices` snapshot so a transient failure does not drop aliases. */
let lastArchive: Utils.DeviceArchiveRow[] = [];
let lastArchiveWarnAt = 0;
const ARCHIVE_WARN_INTERVAL_MS = 30_000;

/**
 * Fetch device archive rows. On failure, reuse the last good snapshot and
 * rate-limit a warning toast instead of silently clearing display names.
 */
export async function fetchDeviceArchive(
    api: ApiClient | undefined | null,
    opts?: {
        toast?: Toast;
        t?: (key: string) => string;
    },
): Promise<Utils.DeviceArchiveRow[]> {
    if (!api) {
        return lastArchive;
    }
    try {
        const rows = await api.list_devices();
        lastArchive = rows || [];
        return lastArchive;
    } catch (e) {
        console.warn('failed to load device archive; keeping last snapshot', e);
        const now = Date.now();
        if (opts?.toast && opts?.t && now - lastArchiveWarnAt >= ARCHIVE_WARN_INTERVAL_MS) {
            lastArchiveWarnAt = now;
            opts.toast.add({
                severity: 'warn',
                summary: opts.t('web.device.archive_load_failed'),
                life: TOAST_LIFE.warn,
            });
        }
        return lastArchive;
    }
}

/** Load online machines + archive and merge into DeviceInfo list. */
export async function loadMergedDevices(
    api: ApiClient | undefined | null,
    opts?: {
        toast?: Toast;
        t?: (key: string) => string;
    },
): Promise<Utils.DeviceInfo[]> {
    if (!api) {
        return [];
    }
    const [resp, archived] = await Promise.all([
        api.list_machines(),
        fetchDeviceArchive(api, opts),
    ]);
    const online = (resp || []).map((device: any) => Utils.buildDeviceInfo(device));
    return Utils.mergeDevicesWithArchive(online, archived);
}

/** Test helper: reset module cache. */
export function resetDeviceArchiveCacheForTests() {
    lastArchive = [];
    lastArchiveWarnAt = 0;
}
