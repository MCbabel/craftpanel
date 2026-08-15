import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { setLocale, startLocale, useLocale } from './locale'

const { locale } = useLocale()

const page = { documentElement: { lang: '' } }

function remembers(start: Record<string, string> = {}) {
	const kept: Record<string, string> = { ...start }
	vi.stubGlobal('localStorage', {
		getItem: (key: string) => kept[key] ?? null,
		setItem: (key: string, value: string) => {
			kept[key] = value
		},
	})
	return kept
}

function remembersNothing() {
	vi.stubGlobal('localStorage', {
		getItem: () => {
			throw new Error('the user has forbidden storage')
		},
		setItem: () => {
			throw new Error('the user has forbidden storage')
		},
	})
}

function speaks(...languages: string[]) {
	vi.stubGlobal('navigator', { language: languages[0], languages })
}

beforeEach(() => {
	page.documentElement.lang = ''
	vi.stubGlobal('document', page)
})

afterEach(() => vi.unstubAllGlobals())

describe('startLocale', () => {
	it('takes the remembered language and not the one of the browser', () => {
		remembers({ 'craftpanel.locale': 'de-DE' })
		speaks('en-US')

		startLocale()

		expect(locale.value).toBe('de-DE')
		expect(page.documentElement.lang).toBe('de-DE')
	})

	it('takes the language of the browser on the first load', () => {
		remembers()
		speaks('de-DE', 'en-US')

		startLocale()

		expect(locale.value).toBe('de-DE')
		expect(page.documentElement.lang).toBe('de-DE')
	})

	it('recognises a variant such as German from Austria as well', () => {
		remembers()
		speaks('de-AT')

		startLocale()

		expect(locale.value).toBe('de-DE')
	})

	it('falls back to English when the browser speaks none of our languages', () => {
		remembers()
		speaks('fr-FR', 'it-IT')

		startLocale()

		expect(locale.value).toBe('en-US')
		expect(page.documentElement.lang).toBe('en-US')
	})

	it('takes the first language of the list that we know', () => {
		remembers()
		speaks('fr-FR', 'de-DE', 'en-US')

		startLocale()

		expect(locale.value).toBe('de-DE')
	})

	it('passes over a remembered language that we do not have', () => {
		remembers({ 'craftpanel.locale': 'kl-GL' })
		speaks('en-US')

		startLocale()

		expect(locale.value).toBe('en-US')
	})

	it('gets by without storage', () => {
		remembersNothing()
		speaks('de-DE')

		startLocale()

		expect(locale.value).toBe('de-DE')
	})
})

describe('setLocale', () => {
	it('remembers the choice for the next load', () => {
		const kept = remembers()
		speaks('en-US')
		startLocale()

		setLocale('de-DE')

		expect(locale.value).toBe('de-DE')
		expect(kept['craftpanel.locale']).toBe('de-DE')
		expect(page.documentElement.lang).toBe('de-DE')

		startLocale()

		expect(locale.value).toBe('de-DE')
	})

	it('lets through only the languages we have', () => {
		remembers()
		speaks('en-US')
		startLocale()

		setLocale('ja-JP')

		expect(locale.value).toBe('en-US')
		expect(page.documentElement.lang).toBe('en-US')
	})

	it('stays with the choice even when storage does not keep it', () => {
		remembersNothing()
		speaks('en-US')
		startLocale()

		setLocale('de-DE')

		expect(locale.value).toBe('de-DE')
	})
})
