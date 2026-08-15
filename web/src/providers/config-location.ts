import type { ContentProjectType } from '@/api'

export interface ConfigEntry {
	name: string
	type: 'file' | 'directory' | 'symlink'
}

export interface ConfigItem {
	contentType: ContentProjectType
	filePath: string
	fileName: string
	title: string | null
	slug: string | null
}

export type ConfigLocation =
	| { kind: 'own'; path: string; editing?: string }
	| { kind: 'shared'; path: string }
	| { kind: 'none'; path: string }

const JAR = /\.jar(\.disabled)?$/i

function directoryOf(path: string): string {
	const cut = path.lastIndexOf('/')
	const directory = cut < 0 ? '' : path.slice(0, cut)
	return directory.startsWith('/') ? directory : `/${directory}`
}

export function configRoot(contentType: ContentProjectType, filePath: string): string | null {
	if (contentType === 'plugin') return directoryOf(filePath)
	if (contentType === 'mod') return '/config'
	return null
}

function normalized(value: string): string {
	return value.toLowerCase().replace(/[^a-z0-9]+/g, '')
}

function nameOf(entry: ConfigEntry): string {
	return normalized(entry.type === 'file' ? entry.name.replace(/\.[^.]+$/, '') : entry.name)
}

function join(root: string, name: string): string {
	return root === '/' ? `/${name}` : `${root}/${name}`
}

export function configLocation(
	item: ConfigItem,
	listings: ReadonlyMap<string, readonly ConfigEntry[]>,
): ConfigLocation | null {
	const root = configRoot(item.contentType, item.filePath)
	if (root === null) return null
	const entries = listings.get(root)
	if (entries === undefined) return null

	const names = new Set(
		[item.title, item.slug, item.fileName.replace(JAR, '')]
			.map((value) => normalized(value ?? ''))
			.filter((value) => value !== ''),
	)
	const own =
		entries.find((entry) => entry.type === 'directory' && names.has(nameOf(entry))) ??
		entries.find(
			(entry) => entry.type === 'file' && !JAR.test(entry.name) && names.has(nameOf(entry)),
		)
	if (own !== undefined) {
		return own.type === 'directory'
			? { kind: 'own', path: join(root, own.name) }
			: { kind: 'own', path: root, editing: own.name }
	}

	const written = entries.some((entry) => entry.type !== 'file' || !JAR.test(entry.name))
	return written ? { kind: 'shared', path: root } : { kind: 'none', path: root }
}

export function configPath(spot: ConfigLocation): string {
	return spot.kind === 'own' && spot.editing !== undefined
		? join(spot.path, spot.editing)
		: spot.path
}
