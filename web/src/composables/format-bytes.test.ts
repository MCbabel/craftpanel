import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { IntlMessageFormat } from 'intl-messageformat'
import { describe, expect, it, vi } from 'vitest'

vi.mock('@modrinth/ui', () => ({
	defineMessage: (descriptor: unknown) => descriptor,
	useVIntl: () => ({
		formatMessage: (
			descriptor: { defaultMessage: string },
			values: Record<string, unknown> = {},
		) => new IntlMessageFormat(descriptor.defaultMessage, 'en-US').format(values) as string,
	}),
}))

const { useFormatDecimalBytes } = await import('./format-bytes')

describe('Bytes in the unit Google publishes', () => {
	const format = useFormatDecimalBytes()

	it('writes the daily ceiling the way Google writes it', () => {
		expect(format(750_000_000_000)).toBe('750 GB')
	})

	it('does not reach for the next unit before the thousand is full', () => {
		expect(format(999)).toBe('999 bytes')
		expect(format(1_000)).toBe('1 kB')
		expect(format(999_999_999)).toBe('1,000 MB')
		expect(format(1_000_000_000)).toBe('1 GB')
	})

	it('counts a part of the day in the same unit as the ceiling', () => {
		expect(format(375_000_000_000)).toBe('375 GB')
		expect(format(1_500_000_000)).toBe('1.5 GB')
	})

	it('says nothing rather than 0.00 kB for an account that has sent nothing', () => {
		expect(format(0)).toBe('0 bytes')
	})

	it('stops at terabytes, the largest unit it names', () => {
		expect(format(5_000_000_000_000)).toBe('5 TB')
		expect(format(2_000_000_000_000_000)).toBe('2,000 TB')
	})
})

describe('Where Google’s day figure is shown', () => {
	const ROOT = resolve(import.meta.dirname, '../../..')

	const PAGES = [
		{ file: 'web/src/pages/account/Drive.vue', day: /view\.day\.\w+Bytes/g },
		{
			file: 'web/src/pages/admin/Drive.vue',
			day: /account\.(?:uploaded_today|daily_upload_limit)_bytes/g,
		},
	]

	it.each(PAGES)('counts the day in 1000s on $file', ({ file, day }) => {
		const source = readFileSync(resolve(ROOT, file), 'utf8')
		const figures = [...source.matchAll(day)]

		expect(figures.length, 'the page shows no day figure at all').toBeGreaterThan(0)
		for (const figure of figures) {
			const around = source.slice(Math.max(0, figure.index - 60), figure.index)
			expect(around, `${figure[0]} is formatted in 1024s and would read as GiB`).toContain(
				'decimalBytes(',
			)
		}
	})
})
