import {
	CloudIcon,
	CoffeeIcon,
	GlobeIcon,
	MailIcon,
	SettingsIcon,
	UserPlusIcon,
	UsersIcon,
} from '@modrinth/assets'
import { defineMessages, type MessageDescriptor } from '@modrinth/ui'
import type { Component } from 'vue'
import type { RouteRecordSingleView } from 'vue-router'

export interface AdminPage {
	name: string
	path: string
	component: RouteRecordSingleView['component']
	label: MessageDescriptor | null
	icon: Component | null
}

const messages = defineMessages({
	users: { id: 'panel.menu.users', defaultMessage: 'Users' },
	playit: { id: 'panel.menu.playit-all', defaultMessage: 'Public addresses (all users)' },
	mail: { id: 'panel.menu.mail', defaultMessage: 'Mail' },
	registrations: { id: 'panel.menu.registrations', defaultMessage: 'Sign-ups' },
	drive: { id: 'panel.menu.drive', defaultMessage: 'Google Drive' },
	runtimes: { id: 'panel.menu.runtimes', defaultMessage: 'Java runtimes' },
	settings: { id: 'panel.menu.panel-settings', defaultMessage: 'Panel settings' },
})

export const adminPages: AdminPage[] = [
	{
		name: 'admin-users',
		path: 'admin/users',
		component: () => import('@/pages/admin/Users.vue'),
		label: messages.users,
		icon: UsersIcon,
	},
	{
		name: 'admin-playit',
		path: 'admin/playit',
		component: () => import('@/pages/admin/Playit.vue'),
		label: messages.playit,
		icon: GlobeIcon,
	},
	{
		name: 'admin-mail',
		path: 'admin/mail',
		component: () => import('@/pages/admin/Mail.vue'),
		label: messages.mail,
		icon: MailIcon,
	},
	{
		name: 'admin-registrations',
		path: 'admin/registrations',
		component: () => import('@/pages/admin/Registrations.vue'),
		label: messages.registrations,
		icon: UserPlusIcon,
	},
	{
		name: 'admin-drive',
		path: 'admin/drive',
		component: () => import('@/pages/admin/Drive.vue'),
		label: messages.drive,
		icon: CloudIcon,
	},
	{
		name: 'admin-runtimes',
		path: 'admin/runtimes',
		component: () => import('@/pages/admin/Runtimes.vue'),
		label: messages.runtimes,
		icon: CoffeeIcon,
	},
	{
		name: 'admin-settings',
		path: 'admin/settings',
		component: () => import('@/pages/admin/Settings.vue'),
		label: messages.settings,
		icon: SettingsIcon,
	},
]
