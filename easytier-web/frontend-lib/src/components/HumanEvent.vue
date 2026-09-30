<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { EventType } from '../types/network'
import { computed } from 'vue';
import { Tag } from 'primevue';

const props = defineProps<{
  event: {
    [key: string]: any
  }
}>()
const { t } = useI18n()

const eventKey = computed(() => {
  const key = Object.keys(props.event)[0]
  return Object.keys(EventType).includes(key) ? key : 'Unknown'
})

const eventValue = computed(() => props.event[eventKey.value])

const severity = computed(() => {
  const key = eventKey.value
  if (/Error|Failed|Conflicted|Removed|Disconnected/i.test(key)) return 'danger'
  if (/Added|Connected|Ready|Started|Accepted/i.test(key)) return 'success'
  if (/Connecting|Changed|Updated/i.test(key)) return 'info'
  return 'secondary'
})

const detailText = computed(() => {
  const value = eventValue.value
  if (eventKey.value === 'DhcpIpv4Changed' && Array.isArray(value) && value.length >= 2) {
    return `${value[0]} → ${value[1]}`
  }
  if (value == null) return '—'
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    return String(value)
  }
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
})
</script>

<template>
  <div class="human-event">
    <Tag class="human-event-tag" :severity="severity" :value="t(`event.${eventKey}`)" />
    <pre class="human-event-detail">{{ detailText }}</pre>
  </div>
</template>

<style scoped>
.human-event {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 0.5rem;
  min-width: 0;
}

.human-event-tag {
  max-width: 100%;
}

.human-event-detail {
  margin: 0;
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  padding: 0.65rem 0.75rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: calc(var(--et-radius, 0.75rem) - 0.25rem);
  background: var(--surface-50, #f8fafc);
  color: var(--text-color, #1e293b);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.75rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: auto;
}

@media (prefers-color-scheme: dark) {
  .human-event-detail {
    background: var(--surface-100, #1e293b);
  }
}
</style>
