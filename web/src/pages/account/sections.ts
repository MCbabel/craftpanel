import type { Component } from 'vue'

import AccountDrive from './Drive.vue'
import AccountPlayit from './Playit.vue'

export interface AccountSection {
	id: string
	component: Component
}

export const accountSections: AccountSection[] = [
	{ id: 'playit', component: AccountPlayit },
	{ id: 'drive', component: AccountDrive },
]
