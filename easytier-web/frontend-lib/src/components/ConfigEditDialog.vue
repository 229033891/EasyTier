<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { NetworkConfig } from '../types/network';
import { Divider, Button, Dialog, Textarea } from 'primevue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

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
        errorMessage.value = '';
        const generated = await props.generateConfig(config);
        if (sequence !== generateSequence || !visible.value || curNetwork.value !== config || tomlConfig.value !== previousToml) {
            return;
        }
        tomlConfig.value = generated;
    } catch (e) {
        if (sequence !== generateSequence || !visible.value || curNetwork.value !== config) {
            return;
        }
        errorMessage.value = 'Failed to generate config: ' + (e instanceof Error ? e.message : String(e));
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
    } catch (e) {
        errorMessage.value = 'Failed to save config: ' + (e instanceof Error ? e.message : String(e));
    } finally {
        saveInProgress = false;
    }
};

const tomlConfig = ref<string>('')
const tomlConfigRows = ref<number>(1);
const errorMessage = ref<string>('');

watch(tomlConfig, (newValue) => {
    tomlConfigRows.value = newValue.split('\n').length;
    errorMessage.value = '';
});

</script>
<template>
    <Dialog v-model:visible="visible" modal :header="t('config_file')" :style="{ width: '70%' }">
        <pre v-if="errorMessage"
            class="mb-2 p-2 rounded text-sm overflow-auto bg-red-100 text-red-700 max-h-40">{{ errorMessage }}</pre>
        <div class="flex w-full" style="max-height: 60vh; overflow-y: auto;">
            <Textarea v-model="tomlConfig" class="w-full h-full font-mono flex flex-col resize-none" :rows="tomlConfigRows"
                spellcheck="false" :readonly="props.readonly"></Textarea>
        </div>
        <Divider />
        <div class="flex gap-2 justify-end">
            <Button v-if="!props.readonly" type="button" :label="t('save')" @click="handleConfigSave" />
            <Button type="button" :label="t('close')" @click="visible = false" />
        </div>
    </Dialog>
</template>
