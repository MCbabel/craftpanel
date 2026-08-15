import { describe, expect, it } from 'vitest'

import { publicPages } from '@/pages/auth/routes'
import { decideRoute, type Visitor } from '@/router-guard'

const stranger = null
const plain: Visitor = { isAdmin: false, mustChangePassword: false }
const admin: Visitor = { isAdmin: true, mustChangePassword: false }
const freshly: Visitor = { isAdmin: false, mustChangePassword: true }

describe('decideRoute', () => {
	it('lets every public page through without a session', () => {
		expect(publicPages.length).toBeGreaterThan(0)
		for (const page of publicPages) {
			expect(decideRoute({ name: page.name, adminOnly: false }, stranger)).toBe('allow')
		}
	})

	it('sends every other page to the sign-in without a session', () => {
		for (const name of ['servers', 'account', 'server-backups', 'change-password', '']) {
			expect(decideRoute({ name, adminOnly: false }, stranger)).toBe('to-login')
		}
	})

	it('sends a signed-in visitor away from the forms that are none of their business', () => {
		for (const page of publicPages.filter((entry) => entry.whenSignedIn === 'bounce')) {
			expect(decideRoute({ name: page.name, adminOnly: false }, plain)).toBe('bounce-signed-in')
		}
	})

	it('leaves a signed-in visitor standing on a page that redeems a token', () => {
		for (const page of publicPages.filter((entry) => entry.whenSignedIn === 'allow')) {
			expect(decideRoute({ name: page.name, adminOnly: false }, plain)).toBe('allow')
			expect(decideRoute({ name: page.name, adminOnly: false }, freshly)).toBe('allow')
		}
	})

	it('holds the forced password change everywhere but on its own page', () => {
		expect(decideRoute({ name: 'servers', adminOnly: false }, freshly)).toBe('to-change-password')
		expect(decideRoute({ name: 'change-password', adminOnly: false }, freshly)).toBe('allow')
	})

	it('gives admin pages to an admin only', () => {
		expect(decideRoute({ name: 'admin-users', adminOnly: true }, plain)).toBe('to-servers')
		expect(decideRoute({ name: 'admin-users', adminOnly: true }, admin)).toBe('allow')
		expect(
			decideRoute({ name: 'admin-users', adminOnly: true }, { ...freshly, isAdmin: true }),
		).toBe('to-change-password')
	})
})
