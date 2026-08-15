import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

import { bounceWhenSignedIn, publicPages, publicRouteNames } from '@/pages/auth/routes'
import { decideRoute } from '@/router-guard'

const EXPECTED = [
	{ name: 'login', path: '/login', whenSignedIn: 'bounce' },
	{ name: 'register', path: '/register', whenSignedIn: 'bounce' },
	{ name: 'verify-email', path: '/verify-email', whenSignedIn: 'allow' },
	{ name: 'registration-pending', path: '/registration-pending', whenSignedIn: 'allow' },
	{ name: 'forgot-password', path: '/forgot-password', whenSignedIn: 'bounce' },
	{ name: 'reset-password', path: '/reset-password', whenSignedIn: 'allow' },
] as const

const ROOT = resolve(import.meta.dirname, '../../../..')

const WAY_IN: Record<(typeof EXPECTED)[number]['name'], readonly string[]> = {
	login: ['web/src/router.ts'],
	register: ['web/src/pages/auth/Login.vue'],
	'verify-email': ['crates/craftpanel/src/mail/message.rs'],
	'registration-pending': ['web/src/pages/auth/VerifyEmail.vue', 'web/src/pages/auth/Login.vue'],
	'forgot-password': ['web/src/pages/auth/Login.vue', 'web/src/pages/auth/NewPassword.vue'],
	'reset-password': ['crates/craftpanel/src/mail/message.rs'],
}

function leadsTo(source: string, page: { name: string; path: string }): boolean {
	return (
		source.includes(`name: '${page.name}'`) ||
		source.includes(`('${page.name}'`) ||
		source.includes(`${page.path}#`)
	)
}

describe('the session-free pages', () => {
	it('are exactly these six, with these paths', () => {
		expect(publicPages.map((page) => ({ name: page.name, path: page.path }))).toEqual(
			EXPECTED.map((page) => ({ name: page.name, path: page.path })),
		)
	})

	it('lets every one of them through without a session', () => {
		for (const page of EXPECTED) {
			expect(publicRouteNames.has(page.name)).toBe(true)
			expect(decideRoute({ name: page.name, adminOnly: false }, null)).toBe('allow')
		}
	})

	it('sends a signed-in visitor away from the forms only, never from a redemption', () => {
		for (const page of EXPECTED) {
			expect(bounceWhenSignedIn.has(page.name)).toBe(page.whenSignedIn === 'bounce')
		}
		expect(bounceWhenSignedIn.has('verify-email')).toBe(false)
		expect(bounceWhenSignedIn.has('reset-password')).toBe(false)
	})

	it('has a way in that is not typing it out', () => {
		for (const page of EXPECTED) {
			const ways = WAY_IN[page.name]
			expect(ways.length, `no way to ${page.path} written down`).toBeGreaterThan(0)
			for (const way of ways) {
				const source = readFileSync(resolve(ROOT, way), 'utf8')
				expect(leadsTo(source, page), `${way} does not lead to ${page.path}`).toBe(true)
			}
		}
	})
})
