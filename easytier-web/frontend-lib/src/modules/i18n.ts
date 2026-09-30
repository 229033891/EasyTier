import { createI18n } from 'vue-i18n'
import type { Locale } from 'vue-i18n'

import EnLocale from '../locales/en.yaml'
import CnLocale from '../locales/cn.yaml'

// Import i18n resources
// https://vitejs.dev/guide/features.html#glob-import
export const i18n = createI18n({
  legacy: false,
  locale: '',
  fallbackLocale: '',
  messages: {},
})

const localesMap = {
  "en": EnLocale,
  "cn": CnLocale,
} as Record<string, any>

export const availableLocales = Object.keys(localesMap)

const loadedLanguages: string[] = []

function normalizeLanguage(lang: string | null | undefined): string {
  return lang && availableLocales.includes(lang) ? lang : 'en'
}

export function toggleLanguage() {
  const currentLang = normalizeLanguage(localStorage.getItem('lang'))
  const newLang = currentLang === 'en' ? 'cn' : 'en'
  void loadLanguageAsync(newLang)
}

export function getCurrentLanguage() {
  return normalizeLanguage(localStorage.getItem('lang'))
}

function setI18nLanguage(lang: Locale) {
  i18n.global.locale.value = lang as any
  localStorage.setItem('lang', lang)
  return lang
}

export async function loadLanguageAsync(lang: string): Promise<Locale> {
  const normalizedLang = normalizeLanguage(lang)

  // If the same language
  if (i18n.global.locale.value === normalizedLang)
    return setI18nLanguage(normalizedLang)

  // If the language was already loaded
  if (loadedLanguages.includes(normalizedLang))
    return setI18nLanguage(normalizedLang)

  const messages = localesMap[normalizedLang]
  i18n.global.setLocaleMessage(normalizedLang, messages)
  loadedLanguages.push(normalizedLang)
  return setI18nLanguage(normalizedLang)
}

export default {
  i18n,
  localesMap,
  loadLanguageAsync,
  toggleLanguage,
  getCurrentLanguage,
}
