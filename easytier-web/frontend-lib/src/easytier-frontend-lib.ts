import './style.css'

import type { App } from 'vue';
import { Config, Status, ConfigEditDialog, RemoteManagement, HumanEvent, DnsCoverageBadge, DnsHostsEditor, DnsForwardersEditor, DnsUpstreamEditor, LoggingSettingsDialog } from "./components";
import PrimeVue from 'primevue/config'

import I18nUtils from './modules/i18n'
import * as NetworkTypes from './types/network'

// do not use primevue tooltip, it has serious memory leak issue
// https://github.com/primefaces/primevue/issues/5856
import { tooltipDirective, tooltipDefaults, normalizeTooltipValue } from './modules/tooltip';
import { EasyTierPreset, ET_PRIMARY, ET_PRIMARY_EMPHASIS } from './modules/theme';

import * as Api from './modules/api';
import * as Utils from './modules/utils';
import * as DnsCoverage from './modules/dnsCoverage';
import { TOAST_LIFE } from './modules/toast';
import { normalizeLoggerLevel, loggerLevelToRpc } from './modules/logging';

export default {
    install: (app: App): void => {
        app.use(I18nUtils.i18n, { useScope: 'global' })
        app.use(PrimeVue, {
            theme: {
                preset: EasyTierPreset,
                options: {
                    prefix: 'p',
                    darkModeSelector: 'system',
                    cssLayer: {
                        name: 'primevue',
                        order: 'tailwind-base, primevue, tailwind-utilities'
                    }
                },
            },
            zIndex: {
                modal: 1100,        //dialog, drawer
                overlay: 1200,      //select, popover
                menu: 1300,         //overlay menus
                tooltip: 1400       //tooltip
            }
        });

        app.component('Config', Config);
        app.component('ConfigEditDialog', ConfigEditDialog);
        app.component('Status', Status);
        app.component('HumanEvent', HumanEvent);
        app.component('RemoteManagement', RemoteManagement);
        app.directive('tooltip', tooltipDirective);
    }
};

export {
    Config,
    ConfigEditDialog,
    RemoteManagement,
    Status,
    HumanEvent,
    DnsCoverageBadge,
    DnsHostsEditor,
    DnsForwardersEditor,
    DnsUpstreamEditor,
    LoggingSettingsDialog,
    I18nUtils,
    NetworkTypes,
    Api,
    Utils,
    DnsCoverage,
    TOAST_LIFE,
    normalizeLoggerLevel,
    loggerLevelToRpc,
    tooltipDirective,
    tooltipDefaults,
    normalizeTooltipValue,
    EasyTierPreset,
    ET_PRIMARY,
    ET_PRIMARY_EMPHASIS,
};

export type { LogFileInfo, LoggerLevelState, LoggingSettingsApi } from './modules/logging';
