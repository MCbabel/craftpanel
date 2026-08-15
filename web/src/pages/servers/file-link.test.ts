import { describe, expect, it } from 'vitest'

import { fileToOpen, folderToOpen } from './file-link'

describe('address → file to open', () => {
	it('opens the file that Modrinth\'s properties page links to', () => {
		expect(fileToOpen({ path: '/', editing: 'server.properties' })).toEqual({
			name: 'server.properties',
			path: '/server.properties',
			directory: '/',
		})
	})

	it('puts the name onto the folder from the address', () => {
		expect(fileToOpen({ path: '/config', editing: 'paper-global.yml' })).toEqual({
			name: 'paper-global.yml',
			path: '/config/paper-global.yml',
			directory: '/config',
		})
	})

	it('takes a path in `editing` for what it is', () => {
		expect(fileToOpen({ path: '/', editing: 'config/paper-global.yml' })?.path).toBe(
			'/config/paper-global.yml',
		)
		expect(fileToOpen({ editing: '/plugins/config.yml' })).toEqual({
			name: 'config.yml',
			path: '/plugins/config.yml',
			directory: '/plugins',
		})
	})

	it('names the folder the file really sits in', () => {
		expect(fileToOpen({ path: '/', editing: 'config/paper-global.yml' })?.directory).toBe('/config')
	})

	it('leaves the page alone when nobody wants to open anything', () => {
		expect(fileToOpen({})).toBeNull()
		expect(fileToOpen({ path: '/config' })).toBeNull()
		expect(fileToOpen({ editing: '' })).toBeNull()
		expect(fileToOpen({ editing: null })).toBeNull()
		expect(fileToOpen({ editing: '/' })).toBeNull()
	})

	it('takes the first one when the parameter comes twice', () => {
		expect(fileToOpen({ editing: ['server.properties', 'ops.json'] })?.name).toBe(
			'server.properties',
		)
	})

	it('cannot be led out of the server directory with `..`', () => {
		expect(fileToOpen({ path: '/', editing: '../../etc/passwd' })?.path).toBe('/etc/passwd')
		expect(fileToOpen({ path: '/config/../world', editing: 'level.dat' })?.path).toBe(
			'/world/level.dat',
		)
	})
})

describe('address → folder to open', () => {
	it('takes the path when nobody wants to open a file', () => {
		expect(folderToOpen({ path: '/plugins/WorldEdit' })).toBe('/plugins/WorldEdit')
		expect(folderToOpen({ path: '/plugins' })).toBe('/plugins')
	})

	it('leaves a link that names a file to the editor', () => {
		expect(folderToOpen({ path: '/', editing: 'server.properties' })).toBeNull()
	})

	it('leaves the page alone when there is no path', () => {
		expect(folderToOpen({})).toBeNull()
		expect(folderToOpen({ path: '' })).toBeNull()
		expect(folderToOpen({ path: null })).toBeNull()
	})

	it('cannot be led out of the server directory with `..`', () => {
		expect(folderToOpen({ path: '/plugins/../../etc' })).toBe('/etc')
	})
})
