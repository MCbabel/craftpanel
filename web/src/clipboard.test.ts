import { afterEach, describe, expect, it, vi } from 'vitest'

import { copyBySelection, installClipboardFallback, setCopyFailureHandler } from './clipboard'

interface Shimmed {
	clipboard?: { writeText(text: string): Promise<void> }
}

function fakePage(execCommand: () => boolean) {
	const log: string[] = []
	const range = { what: 'the selection of the user' }
	const selection = {
		rangeCount: 1,
		getRangeAt: () => range,
		removeAllRanges: () => log.push('selection cleared'),
		addRange: (restored: unknown) =>
			log.push(restored === range ? 'selection back' : 'foreign selection set'),
	}
	const field = {
		value: '',
		style: {} as Record<string, string>,
		setAttribute: () => undefined,
		select: () => log.push('field selected'),
		remove: () => log.push('field removed'),
	}
	const page = {
		activeElement: { focus: () => log.push('focus back') },
		body: { append: () => log.push('field hooked in') },
		createElement: () => field,
		getSelection: () => selection,
		execCommand: (command: string) => {
			log.push(`execCommand ${command}`)
			return execCommand()
		},
	}

	return { page: page as unknown as Document, field, log }
}

afterEach(() => setCopyFailureHandler(null))

describe('The fallback by way of the selection', () => {
	it('puts the text into a field, copies it and tidies up behind itself', () => {
		const { page, field, log } = fakePage(() => true)

		expect(copyBySelection('craftpanel.local:25565', page)).toBe(true)

		expect(field.value).toBe('craftpanel.local:25565')
		expect(log).toEqual([
			'field hooked in',
			'field selected',
			'execCommand copy',
			'field removed',
			'selection cleared',
			'selection back',
			'focus back',
		])
	})

	it('places the field so that nobody sees it and the page does not jump', () => {
		const { page, field } = fakePage(() => true)

		copyBySelection('x', page)

		expect(field.style.position).toBe('fixed')
		expect(field.style.opacity).toBe('0')
	})

	it('says no when the browser refuses to copy', () => {
		const { page } = fakePage(() => false)

		expect(copyBySelection('x', page)).toBe(false)
	})

	it('leaves behind neither the field nor the selection when execCommand throws', () => {
		const { page, log } = fakePage(() => {
			throw new Error('unsupported')
		})

		expect(copyBySelection('x', page)).toBe(false)
		expect(log).toContain('field removed')
		expect(log).toContain('selection back')
		expect(log).toContain('focus back')
	})
})

describe('The hooking in', () => {
	it('does not touch a real clipboard', () => {
		const real = { writeText: () => Promise.resolve() }
		const host: Shimmed = { clipboard: real }

		expect(installClipboardFallback(host, () => true)).toBe(false)
		expect(host.clipboard).toBe(real)
	})

	it('hands out a promise that behaves like the real one', async () => {
		const host: Shimmed = {}
		const copied: string[] = []

		expect(
			installClipboardFallback(host, (text) => {
				copied.push(text)
				return true
			}),
		).toBe(true)

		await expect(host.clipboard?.writeText('01JD…')).resolves.toBeUndefined()
		expect(copied).toEqual(['01JD…'])
	})

	it('rejects and reports it when the fallback copies nothing either', async () => {
		const host: Shimmed = {}
		const seen = vi.fn()
		setCopyFailureHandler(seen)
		installClipboardFallback(host, () => false)

		await expect(host.clipboard?.writeText('x')).rejects.toThrow()
		expect(seen).toHaveBeenCalledOnce()
	})
})
