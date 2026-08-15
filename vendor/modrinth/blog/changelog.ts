import type dayjs from 'dayjs'

export type Product = 'web' | 'hosting' | 'app'

export type VersionEntry = {
	date: dayjs.Dayjs
	product: Product
	version?: string
	body: string
}

export function getChangelog(): VersionEntry[] {
	return []
}
