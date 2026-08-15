import { describe, expect, it } from 'vitest'

import {
	addressFromField,
	type MailReadiness,
	resetLinkProblem,
	sameAddress,
} from '@/pages/admin/users'

type Post = NonNullable<MailReadiness>

const READY: Post = { state: 'configured', link_base: 'https://panel.example' }
const withAddress = { email: 'max@example.test' }
const withoutAddress = { email: null }

function post(over: Partial<Post>): Post {
	return { ...READY, ...over }
}

describe('the button for the reset link (21.4)', () => {
	it('is free when the account and the mail sending both bring their part', () => {
		expect(resetLinkProblem(withAddress, READY)).toBeNull()
	})

	it('names the missing address first, even when the mail sending is missing too', () => {
		expect(resetLinkProblem(withoutAddress, post({ state: 'not_configured' }))).toBe('no-address')
	})

	it('locks without mail sending', () => {
		expect(resetLinkProblem(withAddress, post({ state: 'not_configured' }))).toBe('no-mail')
	})

	it('locks when the panel does not know its own address', () => {
		expect(resetLinkProblem(withAddress, post({ link_base: null }))).toBe('no-link-base')
	})

	it('counts the file sink as set up', () => {
		expect(resetLinkProblem(withAddress, post({ state: 'file_sink' }))).toBeNull()
	})

	it('does not lock while the mail sending is unknown', () => {
		expect(resetLinkProblem(withAddress, null)).toBeNull()
		expect(resetLinkProblem(withoutAddress, null)).toBe('no-address')
	})

	it('locks while the field says something other than the account does', () => {
		expect(resetLinkProblem(withAddress, READY, 'new@example.test')).toBe('unsaved-address')
		expect(resetLinkProblem(withAddress, READY, '')).toBe('unsaved-address')
		expect(resetLinkProblem(withAddress, READY, ' MAX@Example.test ')).toBeNull()
		expect(resetLinkProblem(withoutAddress, READY, '')).toBe('no-address')
	})
})

describe('the address field (12.3, 12.5)', () => {
	it('turns an empty field into the null with which 12.5 clears the address', () => {
		expect(addressFromField('')).toBeNull()
		expect(addressFromField('   ')).toBeNull()
		expect(addressFromField(' max@example.test ')).toBe('max@example.test')
	})

	it('holds two spellings of one address to be the same', () => {
		expect(sameAddress(' MAX@Example.TEST ', 'max@example.test')).toBe(true)
		expect(sameAddress('max@example.test', 'max@example.test')).toBe(true)
		expect(sameAddress('max+1@example.test', 'max@example.test')).toBe(false)
		expect(sameAddress('', null)).toBe(true)
		expect(sameAddress('   ', null)).toBe(true)
		expect(sameAddress('', 'max@example.test')).toBe(false)
		expect(sameAddress('max@example.test', null)).toBe(false)
	})
})
