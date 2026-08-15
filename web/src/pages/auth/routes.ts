import type { RouteRecordSingleView } from 'vue-router'

export interface PublicPage {
	name: string
	path: string
	component: RouteRecordSingleView['component']
	whenSignedIn: 'bounce' | 'allow'
}

export const publicPages: PublicPage[] = [
	{
		name: 'login',
		path: '/login',
		component: () => import('@/pages/auth/Login.vue'),
		whenSignedIn: 'bounce',
	},
	{
		name: 'register',
		path: '/register',
		component: () => import('@/pages/auth/Register.vue'),
		whenSignedIn: 'bounce',
	},
	{
		name: 'verify-email',
		path: '/verify-email',
		component: () => import('@/pages/auth/VerifyEmail.vue'),
		whenSignedIn: 'allow',
	},
	{
		name: 'registration-pending',
		path: '/registration-pending',
		component: () => import('@/pages/auth/RegistrationPending.vue'),
		whenSignedIn: 'allow',
	},
	{
		name: 'forgot-password',
		path: '/forgot-password',
		component: () => import('@/pages/auth/ForgotPassword.vue'),
		whenSignedIn: 'bounce',
	},
	{
		name: 'reset-password',
		path: '/reset-password',
		component: () => import('@/pages/auth/NewPassword.vue'),
		whenSignedIn: 'allow',
	},
]

export const publicRouteNames: ReadonlySet<string> = new Set(publicPages.map((page) => page.name))

export const bounceWhenSignedIn: ReadonlySet<string> = new Set(
	publicPages.filter((page) => page.whenSignedIn === 'bounce').map((page) => page.name),
)
