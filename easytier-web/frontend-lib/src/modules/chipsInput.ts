/**
 * Commit free-typed tokens into a PrimeVue AutoComplete `multiple` model.
 * Used on Enter / comma (and blur) so drafts are not left only in the input DOM.
 */

export type ChipsInputOptions = {
  /** When true, skip Enter commit while the suggestion list is open. */
  typeahead?: boolean
}

export function commitChipTokens(values: string[], raw: string): boolean {
  const next = [...values]
  let changed = false
  for (const part of raw.split(/[,，]+/)) {
    const token = part.trim()
    if (token && !next.includes(token)) {
      next.push(token)
      changed = true
    }
  }
  if (changed) {
    values.splice(0, values.length, ...next)
  }
  return changed
}

function chipsInputFromEvent(event: Event): HTMLInputElement | null {
  const input = event.target as HTMLInputElement | null
  if (!input || input.tagName !== 'INPUT') {
    return null
  }
  return input
}

function suggestionListOpen(input: HTMLInputElement): boolean {
  return input.getAttribute('aria-expanded') === 'true'
}

function clearChipsDraft(input: HTMLInputElement): void {
  input.value = ''
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

export function onChipsKeydown(
  event: KeyboardEvent,
  values: string[] | null | undefined,
  opts?: ChipsInputOptions,
): void {
  if (!Array.isArray(values) || event.isComposing) {
    return
  }
  const isEnter = event.key === 'Enter'
  const isComma = event.key === ',' || event.key === '，'
  if (!isEnter && !isComma) {
    return
  }

  const input = chipsInputFromEvent(event)
  if (!input) {
    return
  }

  // 联想下拉展开时：绝不 stopPropagation，避免拦掉 AutoComplete 内部选中
  if (opts?.typeahead && isEnter && suggestionListOpen(input)) {
    return
  }

  const draft = input.value
  const raw = draft.trim()
  if (!raw) {
    // Whitespace-only draft: still consume the key and clear the input.
    if (draft.length > 0) {
      event.preventDefault()
      event.stopImmediatePropagation()
      clearChipsDraft(input)
    }
    return
  }

  event.preventDefault()
  event.stopImmediatePropagation()

  commitChipTokens(values, raw)
  clearChipsDraft(input)
}

/**
 * Commit any in-progress chip text when the field loses focus.
 * Uses focusout (bubbles) so listeners on the AutoComplete root still see the inner input.
 *
 * Do not wire this on typeahead AutoCompletes: suggestion click often closes the
 * list before focusout, so `aria-expanded` is already false and a partial query
 * would be committed as a chip.
 */
export function onChipsFocusOut(
  event: FocusEvent,
  values: string[] | null | undefined,
): void {
  if (!Array.isArray(values)) {
    return
  }
  const input = chipsInputFromEvent(event)
  if (!input) {
    return
  }
  const draft = input.value
  const raw = draft.trim()
  if (!raw) {
    // Clear leftover spaces so the chip field does not keep a blank draft.
    if (draft.length > 0) {
      clearChipsDraft(input)
    }
    return
  }
  commitChipTokens(values, raw)
  clearChipsDraft(input)
}
