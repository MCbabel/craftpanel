import type { LocationQuery, LocationQueryValue } from 'vue-router'

export interface FileToOpen {
	name: string
	path: string
	directory: string
}

function single(value: LocationQueryValue | LocationQueryValue[] | undefined): string {
	const first = Array.isArray(value) ? value[0] : value
	return typeof first === 'string' ? first : ''
}

function normalize(path: string): string {
	const parts: string[] = []
	for (const part of path.split('/')) {
		if (part === '' || part === '.') continue
		if (part === '..') parts.pop()
		else parts.push(part)
	}
	return `/${parts.join('/')}`
}

export function folderToOpen(query: LocationQuery): string | null {
	if (single(query.editing) !== '') return null
	const path = single(query.path)
	return path === '' ? null : normalize(path)
}

export function fileToOpen(query: LocationQuery): FileToOpen | null {
	const editing = single(query.editing)
	if (editing === '') return null

	const directory = normalize(single(query.path))
	const path = normalize(editing.startsWith('/') ? editing : `${directory}/${editing}`)
	const segments = path.split('/').slice(1)
	const name = segments[segments.length - 1] ?? ''
	if (name === '') return null

	return { name, path, directory: normalize(segments.slice(0, -1).join('/')) }
}
