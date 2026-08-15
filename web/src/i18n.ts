import { buildLocaleMessages, createMessageCompiler, type CrowdinMessages } from '@modrinth/ui'
import { uiLocaleModulesEager } from '@modrinth/ui/src/locales.eager.ts'
import { type App, watch } from 'vue'
import { createI18n } from 'vue-i18n'
import { I18N_INJECTION_KEY, type I18nContext } from '@modrinth/ui'

import { setLocale, useLocale } from '@/composables/locale'

const appLocales = import.meta.glob<{ default: CrowdinMessages }>('./locales/*/index.json', {
	eager: true,
})

export const i18n = createI18n({
	legacy: false,
	locale: 'en-US',
	fallbackLocale: 'en-US',
	messageCompiler: createMessageCompiler(),
	missingWarn: false,
	fallbackWarn: false,
	messages: buildLocaleMessages(appLocales, uiLocaleModulesEager),
})

export default {
	install(app: App) {
		app.use(i18n)

		const { locale } = useLocale()
		watch(
			locale,
			(next) => {
				i18n.global.locale.value = next
			},
			{ immediate: true },
		)

		const context: I18nContext = {
			locale: i18n.global.locale,
			t: (key, values) => i18n.global.t(key, values ?? {}) as string,
			setLocale,
		}

		app.provide(I18N_INJECTION_KEY, context)
	},
}
