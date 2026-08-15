import 'floating-vue/dist/style.css'
import 'overlayscrollbars/overlayscrollbars.css'
import '@/styles/global.scss'

import { VueQueryPlugin } from '@tanstack/vue-query'
import FloatingVue from 'floating-vue'
import { createApp } from 'vue'

import App from '@/App.vue'
import { installClipboardFallback } from '@/clipboard'
import { startLocale } from '@/composables/locale'
import { startTheme } from '@/composables/theme'
import { overlayScrollbarsDirective } from '@/directives/overlay-scrollbars'
import i18nPlugin from '@/i18n'
import router from '@/router'

startTheme()
startLocale()
installClipboardFallback()

const app = createApp(App)

app.use(VueQueryPlugin)
app.use(router)
app.use(FloatingVue, {
	themes: {
		'ribbit-popout': {
			$extend: 'dropdown',
			placement: 'bottom-end',
			instantMove: true,
			distance: 8,
		},
		'dismissable-prompt': {
			$extend: 'dropdown',
			placement: 'bottom-start',
		},
	},
})
app.use(i18nPlugin)
app.directive('overlay-scrollbars', overlayScrollbarsDirective)

app.mount('#app')
