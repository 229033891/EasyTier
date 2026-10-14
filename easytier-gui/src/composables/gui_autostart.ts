import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { type } from '@tauri-apps/plugin-os'

/**
 * Keep the GUI registered for login autostart when Service mode is active.
 * The Windows service starts the network; the GUI process owns the tray icon.
 */
export async function syncGuiAutostart(wantEnabled: boolean) {
  if (type() === 'android')
    return
  try {
    const on = await isEnabled()
    if (wantEnabled && !on)
      await enable()
    else if (!wantEnabled && on)
      await disable()
  }
  catch (e) {
    console.warn('Failed to sync GUI login autostart', e)
  }
}
