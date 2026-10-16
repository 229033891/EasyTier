/**
 * Commit free-typed tokens into a PrimeVue AutoComplete `multiple` model.
 * Used on Enter / comma (and blur) so drafts are not left only in the input DOM.
 *
 * IMPORTANT: PrimeVue 4 form components (BaseEditableHolder) snapshot the v-model
 * into internal `d_value` data and only refresh it from a shallow
 * `watch(modelValue)` — an in-place Array#splice never fires that watcher, so
 * committed chips would silently never render (input cleared, no chip). The
 * handlers below therefore always ASSIGN a fresh array back to row[prop],
 * replacing the reference, which does fire the watcher and re-renders chips.
 */

export type ChipsInputOptions = {
  /** When true, skip Enter commit while the suggestion list is open. */
  typeahead?: boolean
}

/** Object + property path that backs one AutoComplete chips field. */
export type ChipsRow = object

/** Pure: dedupe-append tokens; returns a NEW array (callers must assign it back). */
export function commitChipTokens(
  values: readonly string[] | null | undefined,
  raw: string,
): string[] {
  const next = [...(values ?? [])]
  for (const part of raw.split(/[,，]+/)) {
    const token = part.trim()
    if (token && !next.includes(token)) {
      next.push(token)
    }
  }
  return next
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

/** Replace row[prop] with a fresh array so PrimeVue's shallow modelValue watch re-renders. */
function commitDraftInto(row: ChipsRow, prop: string, draft: string): string[] {
  const holder = row as Record<string, unknown>
  const current = Array.isArray(holder[prop]) ? (holder[prop] as string[]) : []
  const next = commitChipTokens(current, draft)
  holder[prop] = next
  return next
}

function ensureChipsArray(row: ChipsRow, prop: string): void {
  const holder = row as Record<string, unknown>
  if (!Array.isArray(holder[prop])) {
    holder[prop] = []
  }
}

export function onChipsKeydown(
  event: KeyboardEvent,
  row: ChipsRow,
  prop: string,
  opts?: ChipsInputOptions,
): void {
  if (row == null || event.isComposing) {
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

  // Create the backing array first: reactors reading row[prop] (e.g. the
  // AutoComplete model binding) must never see undefined mid-interaction.
  ensureChipsArray(row, prop)

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

  commitDraftInto(row, prop, raw)
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
  row: ChipsRow,
  prop: string,
): void {
  if (row == null) {
    return
  }
  const input = chipsInputFromEvent(event)
  if (!input) {
    return
  }
  ensureChipsArray(row, prop)
  const draft = input.value
  const raw = draft.trim()
  if (!raw) {
    // Clear leftover spaces so the chip field does not keep a blank draft.
    if (draft.length > 0) {
      clearChipsDraft(input)
    }
    return
  }
  commitDraftInto(row, prop, raw)
  clearChipsDraft(input)
}
