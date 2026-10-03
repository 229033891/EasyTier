<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { NetworkConfig } from '../types/network';
import { Button, Dialog, Textarea, useToast } from 'primevue'
import { TOAST_LIFE } from '../modules/toast'
import { formatApiErrorDetail } from '../modules/utils'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
const toast = useToast()

const props = defineProps({
    readonly: {
        type: Boolean,
        default: false,
    },
    generateConfig: {
        type: Object as () => (config: NetworkConfig) => Promise<string>,
        required: true,
    },
    saveConfig: {
        type: Object as () => (config: string) => Promise<void>,
        required: true,
    },
})

const curNetwork = defineModel('curNetwork', {
    type: Object as () => NetworkConfig | undefined,
    required: true,
})

const visible = defineModel('visible', {
    type: Boolean,
    default: false,
})
let generateSequence = 0
let saveInProgress = false

async function refreshConfig(newVisible: boolean, config: NetworkConfig | undefined) {
    const sequence = ++generateSequence
    if (!newVisible || !config) {
        tomlConfig.value = '';
        return;
    }

    const previousToml = tomlConfig.value;
    try {
        const generated = await props.generateConfig(config);
        if (sequence !== generateSequence || !visible.value || curNetwork.value !== config || tomlConfig.value !== previousToml) {
            return;
        }
        tomlConfig.value = generated;
    } catch (e) {
        if (sequence !== generateSequence || !visible.value || curNetwork.value !== config) {
            return;
        }
        toast.add({
            severity: 'error',
            summary: t('config_generation_failed'),
            detail: formatApiErrorDetail(e, t),
            life: TOAST_LIFE.error,
        })
        tomlConfig.value = '';
    }
}

watch([visible, curNetwork], ([newVisible, newCurNetwork]) => {
    void refreshConfig(newVisible, newCurNetwork);
})
onMounted(() => {
    void refreshConfig(visible.value, curNetwork.value);
});

const handleConfigSave = async () => {
    if (props.readonly || saveInProgress) return;
    saveInProgress = true;
    try {
        await props.saveConfig(tomlConfig.value);
        visible.value = false;
        toast.add({
            severity: 'success',
            summary: t('web.common.success'),
            life: TOAST_LIFE.success,
        })
    } catch (e) {
        toast.add({
            severity: 'error',
            summary: t('web.device_management.save_failed'),
            detail: formatApiErrorDetail(e, t),
            life: TOAST_LIFE.error,
        })
    } finally {
        saveInProgress = false;
    }
};

const tomlConfig = ref<string>('')
const tomlConfigRows = ref<number>(1);

watch(tomlConfig, (newValue) => {
    tomlConfigRows.value = newValue.split('\n').length;
});

</script>
<template>
    <Dialog v-model:visible="visible" modal :header="t('config_file')"
        class="et-dialog et-dialog--wide"
        :style="{ width: 'min(70vw, 56rem)', maxWidth: 'calc(100vw - 2rem)' }">
        <div class="et-dialog-scroll">
            <Textarea v-model="tomlConfig" class="w-full h-full font-mono flex flex-col resize-none" :rows="tomlConfigRows"
                spellcheck="false" :readonly="props.readonly"></Textarea>
        </div>
        <template #footer>
            <Button type="button" severity="secondary" outlined :label="t('close')" @click="visible = false" />
            <Button v-if="!props.readonly" type="button" :label="t('save')" @click="handleConfigSave" />
        </template>
    </Dialog>
</template>

<style scoped>
.et-dialog-scroll {
    max-height: 60vh;
    overflow-y: auto;
    width: 100%;
}
</style>
