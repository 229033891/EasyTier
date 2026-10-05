<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { DnsCoverageState } from '../../modules/dnsCoverage'
import { dnsCoverageBadgeModifier } from '../../modules/dnsCoverage'

const props = defineProps<{
  state: DnsCoverageState | null | undefined
  /** Device list: OS capability only (Magic DNS setting unknown). */
  platformOnly?: boolean
}>()

const { t } = useI18n()

const visible = computed(() => !!props.state && props.state !== 'off')

const label = computed(() =>
  props.state ? t(`dns.coverage.${props.state}`) : '',
)

const tooltip = computed(() => {
  if (!props.state) return ''
  const key = props.platformOnly
    ? `dns.coverage.${props.state}_platform_help`
    : `dns.coverage.${props.state}_help`
  return t(key)
})

const docsUrl = computed(() =>
  props.state === 'manual_required'
    ? t('dns.coverage.manual_required_docs_url')
    : '',
)

const modifier = computed(() =>
  props.state ? dnsCoverageBadgeModifier(props.state) : '',
)
</script>

<template>
  <a
    v-if="visible && docsUrl"
    class="dns-coverage-badge"
    :class="modifier ? `dns-coverage-badge--${modifier}` : undefined"
    :href="docsUrl"
    target="_blank"
    rel="noopener noreferrer"
    v-tooltip.top="tooltip"
    role="link"
  >
    {{ label }}
  </a>
  <span
    v-else-if="visible"
    class="dns-coverage-badge"
    :class="modifier ? `dns-coverage-badge--${modifier}` : undefined"
    v-tooltip.top="tooltip"
    role="status"
  >
    {{ label }}
  </span>
</template>

<style scoped>
.dns-coverage-badge {
  display: inline-flex;
  align-items: center;
  padding: 0.1rem 0.45rem;
  border-radius: 999px;
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  letter-spacing: 0.02em;
  line-height: 1.25;
  white-space: nowrap;
}

.dns-coverage-badge--covered {
  background: var(--et-success, #10b981);
  color: #fff;
}

.dns-coverage-badge--manual_required {
  background: var(--et-warning, #f59e0b);
  color: #1e293b;
  text-decoration: none;
}

a.dns-coverage-badge--manual_required:hover {
  filter: brightness(0.95);
  text-decoration: underline;
}

.dns-coverage-badge--unsupported,
.dns-coverage-badge--version_too_old {
  background: var(--et-danger, #ef4444);
  color: #fff;
}
</style>
