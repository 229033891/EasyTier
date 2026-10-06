/**
 * Toggleable PrimeVue Panel headers are clickable as a whole, but clicks on
 * interactive content inside the header (buttons, links, inputs, tooltips,
 * PrimeVue header actions / toggle button) must not flip the collapsed state
 * (otherwise a single click toggles twice and looks like nothing happened).
 *
 * PrimeVue renders the toggle button either as a native `<button>` or as
 * nested spans/divs carrying `p-button` / `p-panel-header-actions` classes
 * and `data-pc-section` attributes, so both `closest()` and the event's
 * composed path are checked.
 */
const BASE_PANEL_HEADER_INTERACTIVE_SELECTORS = [
  'button',
  'a',
  'input',
  'textarea',
  'select',
  '[role="button"]',
  '.p-button',
  '.p-panel-header-actions',
  '[data-pc-section="headeractions"]',
  '[data-pc-section="togglebutton"]',
]

export function isPanelHeaderInteractiveTarget(
  target: HTMLElement | null,
  event: Event,
  extraSelectors: string[] = [],
): boolean {
  const selectors = [...BASE_PANEL_HEADER_INTERACTIVE_SELECTORS, ...extraSelectors].join(', ')
  if (target?.closest(selectors)) {
    return true
  }
  const path = typeof event.composedPath === 'function' ? event.composedPath() : []
  return path.some((node) => {
    if (!(node instanceof HTMLElement)) {
      return false
    }
    const section = node.dataset?.pcSection
    return section === 'headeractions'
      || section === 'togglebutton'
      || node.classList.contains('p-panel-header-actions')
      || node.classList.contains('p-button')
      || node.tagName === 'BUTTON'
  })
}
