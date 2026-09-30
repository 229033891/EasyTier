import Aura from '@primeuix/themes/aura'
import { definePreset } from '@primeuix/themes'

/** 全站统一主色：sky，避免各入口蓝/紫/绿混用 */
export const EasyTierPreset = definePreset(Aura, {
    semantic: {
        primary: {
            50: '{sky.50}',
            100: '{sky.100}',
            200: '{sky.200}',
            300: '{sky.300}',
            400: '{sky.400}',
            500: '{sky.500}',
            600: '{sky.600}',
            700: '{sky.700}',
            800: '{sky.800}',
            900: '{sky.900}',
            950: '{sky.950}',
        },
    },
})

/** CSS / 组件回退用的品牌主色（与 sky-500 一致） */
export const ET_PRIMARY = '#0ea5e9'
export const ET_PRIMARY_EMPHASIS = '#0284c7'
