<script setup lang="ts">
import { Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n'

const { t } = useI18n()


// 定义组件接收的 props
defineProps<{
  device: Utils.DeviceInfo;
  // 可以传入额外的样式类
  containerClass?: string;
  // 是否使用紧凑布局
  compact?: boolean;
}>();

</script>

<template>
  <div :class="['device-details', containerClass, { 'compact': compact }]">
    <div class="detail-item hostname">
      <div class="detail-label">{{ t('web.device.hostname') }}</div>
      <div class="detail-value">{{ device.hostname }}</div>
    </div>
    <div
      v-if="device.reported_hostname && device.reported_hostname !== device.hostname"
      class="detail-item reported-hostname"
    >
      <div class="detail-label">{{ t('web.device.reported_hostname') }}</div>
      <div class="detail-value">{{ device.reported_hostname }}</div>
    </div>
    <div class="detail-item public-ip">
      <div class="detail-label">{{ t('web.device.connection_addr') }}</div>
      <div class="detail-value">{{ device.public_ip }}</div>
    </div>
    <div class="detail-item running-networks">
      <div class="detail-label">{{ t('web.device.networks') }}</div>
      <div class="detail-value">{{ device.running_network_count }}</div>
    </div>
    <div class="detail-item last-report">
      <div class="detail-label">{{ t('web.device.last_report') }}</div>
      <div class="detail-value">{{ device.report_time }}</div>
    </div>
    <div class="detail-item version">
      <div class="detail-label">{{ t('web.device.version') }}</div>
      <div class="detail-value">{{ device.easytier_version }}</div>
    </div>
    <div class="detail-item machine-id">
      <div class="detail-label">{{ t('web.device.machine_id') }}</div>
      <div class="detail-value">
        <span class="machine-id-value" v-tooltip.top="device.machine_id">{{ device.machine_id }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 基础布局 */
.device-details {
  display: grid;
  grid-template-columns: 1fr;
  gap: var(--et-space-3);
}

/* 标准布局的详情项样式 */
.detail-item {
  position: relative;
  border-bottom: 1px solid var(--surface-border, #e2e8f0);
  padding-bottom: var(--et-space-3);
  /* 只过渡悬停真正会变的属性，不用 transition: all */
  transition: background-color 0.2s ease;
  border-radius: calc(var(--et-radius) - 0.5rem);
}

/* 只读信息行的悬停高亮，用于帮助视线横向对齐；触屏上没有「悬停」，直接跳过 */
@media (hover: hover) {
  .detail-item:hover {
    background-color: var(--surface-hover, rgba(245, 247, 250, 0.5));
  }
}

.detail-item:last-child {
  border-bottom: none;
}

.detail-label {
  font-weight: 600;
  color: var(--text-color, #1e293b);
  font-size: var(--et-fs-body);
  margin-bottom: 0.375rem;
  display: flex;
  align-items: center;
}

/* 紧凑布局样式 */
.device-details.compact {
  gap: 0.4rem;
}

.compact .detail-item {
  padding: 0.3rem 0.2rem;
  display: grid;
  grid-template-columns: 40% 60%;
  align-items: center;
}

.compact .detail-label {
  margin-bottom: 0;
}

.detail-label::before {
  content: "";
  display: inline-block;
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background-color: var(--primary-color, var(--et-primary, #0ea5e9));
  margin-right: var(--et-space-2);
}

.detail-value {
  color: var(--text-color-secondary, #64748b);
  word-break: break-all;
  padding-left: var(--et-space-4);
  line-height: 1.4;
  font-size: var(--et-fs-body);
}

/* 紧凑布局的标签和值样式 */
.compact .detail-label::before {
  width: 3px;
  height: 3px;
  margin-right: 0.3rem;
}

.compact .detail-value {
  padding-left: 0.3rem;
  line-height: 1.2;
  font-size: var(--et-fs-meta);
}

/* 字段色点：统一语义 token，避免散落 hex */
.hostname .detail-label::before {
  background-color: var(--primary-color, var(--et-primary, #0ea5e9));
}

.public-ip .detail-label::before {
  background-color: var(--et-success, #10b981);
}

.running-networks .detail-label::before {
  background-color: var(--et-warn, #f59e0b);
}

.last-report .detail-label::before {
  background-color: var(--et-primary-emphasis, #0284c7);
}

.version .detail-label::before {
  background-color: var(--primary-color, var(--et-primary, #0ea5e9));
}

.machine-id .detail-label::before {
  background-color: var(--et-muted, #64748b);
}

/* 机器ID特殊样式 */
.machine-id-value {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: var(--et-fs-meta);
  background-color: var(--surface-ground, #f8fafc);
  color: var(--text-color, #1e293b);
  padding: var(--et-space-1) var(--et-space-2);
  border-radius: calc(var(--et-radius) - 0.5rem);
  border: var(--et-border);
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 紧凑布局下的机器ID样式 */
.compact .machine-id-value {
  font-size: var(--et-fs-meta);
  padding: 0.15rem 0.3rem;
  border-radius: calc(var(--et-radius) - 0.55rem);
}

/* 暗黑模式适配 */
@media (prefers-color-scheme: dark) {
  .detail-item {
    border-bottom: 1px solid var(--surface-border, #334155);
  }

  .detail-item:last-child {
    border-bottom: none;
  }

  @media (hover: hover) {
    .detail-item:hover {
      background-color: var(--surface-hover, rgba(30, 41, 59, 0.4));
    }
  }

  .detail-value {
    color: var(--text-color-secondary, #cbd5e1);
  }

  .detail-label {
    color: var(--text-color, #e2e8f0);
  }

  .machine-id-value {
    background-color: var(--surface-ground, #1e293b);
    color: var(--text-color, #f1f5f9);
    border-color: var(--surface-border, #334155);
  }
}
</style>
