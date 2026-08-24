import { describe, expect, it } from 'vitest'

import type { BackupTarget } from '@/api/drive'

import {
	backupImpossible,
	driveFactsOf,
	needsOwnersDrive,
	noLongerOurs,
	notRestorable,
	openableInDrive,
	targetIsChoosable,
	unconfirmed,
} from './backup-target'

function target(over: Partial<BackupTarget> = {}): BackupTarget {
	return { target: 'local', effective_target: 'local', policy: 'user_choice', reason: 'ok', ...over }
}

describe('The four fields on a row', () => {
	it('reads a row without the fields as local', () => {
		expect(driveFactsOf({ id: 'a', name: 'old' })).toEqual({
			location: 'local',
			state: null,
			verified: null,
			contentChanged: false,
			webLink: null,
		})
		expect(driveFactsOf(null).location).toBe('local')
		expect(driveFactsOf(undefined).location).toBe('local')
		expect(driveFactsOf('nonsense').location).toBe('local')
	})

	it('reads place, state, confirmation and link when they are there', () => {
		const facts = driveFactsOf({
			location: 'drive',
			drive_state: 'present',
			drive_verified: true,
			drive_web_link: 'https://drive.google.com/file/d/abc/view',
		})

		expect(facts).toEqual({
			location: 'drive',
			state: 'present',
			verified: true,
			contentChanged: false,
			webLink: 'https://drive.google.com/file/d/abc/view',
		})
		expect(openableInDrive(facts)).toBe(true)
		expect(notRestorable(facts)).toBe(false)
		expect(unconfirmed(facts)).toBe(false)
		expect(noLongerOurs(facts)).toBe(false)
	})

	it('says out loud which file in the Drive is no longer the backup', () => {
		const swapped = driveFactsOf({
			location: 'drive',
			drive_state: 'present',
			drive_verified: true,
			drive_content_changed: true,
		})
		expect(noLongerOurs(swapped)).toBe(true)
		expect(unconfirmed(swapped)).toBe(false)

		const sound = driveFactsOf({
			location: 'drive',
			drive_state: 'present',
			drive_verified: true,
			drive_content_changed: false,
		})
		expect(noLongerOurs(sound)).toBe(false)

		const olderPanel = driveFactsOf({ location: 'drive', drive_state: 'present' })
		expect(olderPanel.contentChanged).toBe(false)
		expect(noLongerOurs(olderPanel)).toBe(false)
		expect(noLongerOurs(driveFactsOf({ id: 'a' }))).toBe(false)
	})

	it('says out loud which backup nobody has confirmed', () => {
		const nothingChecked = driveFactsOf({
			location: 'drive',
			drive_state: 'present',
			drive_verified: false,
		})
		expect(unconfirmed(nothingChecked)).toBe(true)

		const olderPanel = driveFactsOf({ location: 'drive', drive_state: 'present' })
		expect(olderPanel.verified).toBeNull()
		expect(unconfirmed(olderPanel)).toBe(false)

		const uploading = driveFactsOf({ location: 'drive', drive_state: null, drive_verified: false })
		expect(unconfirmed(uploading)).toBe(false)
		expect(unconfirmed(driveFactsOf({ id: 'a' }))).toBe(false)
	})

	it('does not take a state the contract does not know', () => {
		expect(driveFactsOf({ location: 'drive', drive_state: 'invented' }).state).toBeNull()
	})

	it('has no link for a backup that is still uploading', () => {
		const running = driveFactsOf({ location: 'drive', drive_state: null, drive_web_link: null })

		expect(running.location).toBe('drive')
		expect(openableInDrive(running)).toBe(false)
	})

	it('knows the two states that cannot be restored from', () => {
		expect(notRestorable(driveFactsOf({ location: 'drive', drive_state: 'missing' }))).toBe(true)
		expect(notRestorable(driveFactsOf({ location: 'drive', drive_state: 'trashed' }))).toBe(true)
		expect(notRestorable(driveFactsOf({ location: 'drive', drive_state: 'unreachable' }))).toBe(
			false,
		)
	})
})

describe('The target of the server', () => {
	const cases: { what: string; target: BackupTarget; possible: boolean }[] = [
		{
			what: 'local_only: the operator\'s off switch',
			target: target({ policy: 'local_only', reason: 'policy' }),
			possible: true,
		},
		{
			what: 'drive_only, but the operator has no Google project',
			target: target({ policy: 'drive_only', effective_target: 'drive', reason: 'not_configured' }),
			possible: false,
		},
		{
			what: 'drive_only, but the owner has connected no Drive',
			target: target({ policy: 'drive_only', effective_target: 'drive', reason: 'not_connected' }),
			possible: false,
		},
		{
			what: 'drive_only, and the owner\'s Drive carries it',
			target: target({ policy: 'drive_only', effective_target: 'drive', reason: 'policy' }),
			possible: true,
		},
		{
			what: 'user_choice without a Google project: it runs locally',
			target: target({ reason: 'not_configured' }),
			possible: true,
		},
		{
			what: 'user_choice without a connected Drive: it runs locally',
			target: target({ reason: 'not_connected' }),
			possible: true,
		},
		{ what: 'user_choice, local chosen', target: target(), possible: true },
		{
			what: 'user_choice, Drive chosen and connected',
			target: target({ target: 'drive', effective_target: 'drive' }),
			possible: true,
		},
	]

	it.each(cases)('locks the button exactly when 10.2 refuses: $what', ({ target, possible }) => {
		expect(backupImpossible(target)).toBe(!possible)
	})

	it('locks nothing while there is no answer (22.9 may fail quietly)', () => {
		expect(backupImpossible(null)).toBe(false)
		expect(needsOwnersDrive(null)).toBe(false)
	})

	it('names a way only when the owner\'s Drive is missing', () => {
		expect(
			needsOwnersDrive(
				target({ policy: 'drive_only', effective_target: 'drive', reason: 'not_connected' }),
			),
		).toBe(true)
		expect(
			needsOwnersDrive(
				target({ policy: 'drive_only', effective_target: 'drive', reason: 'not_configured' }),
			),
		).toBe(false)
		expect(
			needsOwnersDrive(target({ policy: 'drive_only', effective_target: 'drive', reason: 'policy' })),
		).toBe(false)
	})

	it('offers the switch only where there really is a choice', () => {
		expect(targetIsChoosable(target())).toBe(true)
		expect(targetIsChoosable(null)).toBe(false)
		expect(targetIsChoosable(target({ policy: 'drive_only', effective_target: 'drive' }))).toBe(
			false,
		)
		expect(targetIsChoosable(target({ policy: 'local_only' }))).toBe(false)
		expect(targetIsChoosable(target({ reason: 'not_configured' }))).toBe(false)
		expect(
			targetIsChoosable(target({ target: 'drive', reason: 'not_connected' })),
		).toBe(true)
	})
})
