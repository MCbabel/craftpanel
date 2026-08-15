import {
	createRouter,
	createWebHistory,
	type RouteLocationRaw,
	type RouteRecordRaw,
	type RouteRecordSingleView,
	START_LOCATION,
} from 'vue-router'

import { setUnauthenticatedHandler } from '@/api'
import { useSession } from '@/composables/session'
import { adminPages } from '@/pages/admin/routes'
import { publicPages } from '@/pages/auth/routes'
import { decideRoute } from '@/router-guard'

declare module 'vue-router' {
	interface RouteMeta {
		admin?: boolean
	}
}

const routes: RouteRecordRaw[] = [
	...publicPages.map(
		(page): RouteRecordSingleView => ({
			path: page.path,
			name: page.name,
			component: page.component,
		}),
	),
	{
		path: '/change-password',
		name: 'change-password',
		component: () => import('@/pages/auth/ChangePassword.vue'),
	},
	{
		path: '/',
		component: () => import('@/layouts/AppShell.vue'),
		children: [
			{
				path: '',
				name: 'servers',
				component: () => import('@/pages/servers/Index.vue'),
			},
			{
				path: 'new',
				name: 'server-new',
				component: () => import('@/pages/servers/New.vue'),
			},
			{
				path: 'servers/:id',
				name: 'server',
				component: () => import('@/layouts/ServerShell.vue'),
				children: [
					{
						path: '',
						name: 'server-overview',
						component: () => import('@/pages/servers/Overview.vue'),
					},
					{
						path: 'content',
						name: 'server-content',
						component: () => import('@/pages/servers/Content.vue'),
					},
					{
						path: 'browse',
						name: 'server-browse',
						component: () => import('@/pages/servers/Browse.vue'),
					},
					{
						path: 'files',
						name: 'server-files',
						component: () => import('@/pages/servers/Files.vue'),
					},
					{
						path: 'backups',
						name: 'server-backups',
						component: () => import('@/pages/servers/Backups.vue'),
					},
					{
						path: 'access',
						name: 'server-access',
						component: () => import('@/pages/servers/Access.vue'),
					},
					{
						path: 'settings/:section?',
						name: 'server-settings',
						component: () => import('@/pages/servers/Settings.vue'),
					},
				],
			},
			{
				path: 'account',
				name: 'account',
				component: () => import('@/pages/account/Account.vue'),
			},
			...adminPages.map(
				(page): RouteRecordSingleView => ({
					path: page.path,
					name: page.name,
					component: page.component,
					meta: { admin: true },
				}),
			),
			{
				path: 'hosting/manage/:id/files',
				redirect: (to) => ({ name: 'server-files', params: { id: to.params.id } }),
			},
			{
				path: ':pathMatch(.*)*',
				name: 'not-found',
				component: () => import('@/pages/NotFound.vue'),
			},
		],
	},
]

const router = createRouter({
	history: createWebHistory(),
	routes,
	scrollBehavior: (_to, _from, saved) => saved ?? { top: 0 },
})

export function internalPath(value: unknown): string | null {
	if (typeof value !== 'string') return null
	return value.startsWith('/') && !value.startsWith('//') ? value : null
}

function withReturn(name: string, from: { fullPath: string }): RouteLocationRaw {
	return from.fullPath === '/' ? { name } : { name, query: { redirect: from.fullPath } }
}

const session = useSession()

const MODRINTH_PROFILE = /^\/(user|organization)\/[^/]+$/

router.beforeEach(async (to) => {
	if (MODRINTH_PROFILE.test(to.path)) {
		window.location.href = `https://modrinth.com${to.path}`
		return false
	}

	const user = await session.resolve()
	const wish = { name: String(to.name ?? ''), adminOnly: to.meta.admin === true }
	const visitor = user
		? { isAdmin: user.panel_role === 'admin', mustChangePassword: user.must_change_password }
		: null

	switch (decideRoute(wish, visitor)) {
		case 'allow':
			return true
		case 'to-login':
			return withReturn('login', to)
		case 'to-change-password':
			return withReturn('change-password', to)
		case 'to-servers':
			return { name: 'servers' }
		case 'bounce-signed-in': {
			const back = internalPath(to.query.redirect)
			const target = back === null ? null : router.resolve(back)
			const loops =
				target === null ||
				decideRoute(
					{ name: String(target.name ?? ''), adminOnly: target.meta.admin === true },
					visitor,
				) === 'bounce-signed-in'
			return loops ? { name: 'servers' } : target
		}
	}
})

setUnauthenticatedHandler(() => {
	session.forget()
	const current = router.currentRoute.value
	if (current === START_LOCATION || current.name === 'login') return
	void router.replace(withReturn('login', current))
})

export default router
