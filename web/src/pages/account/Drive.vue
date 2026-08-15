<template>
	<Card class="!mb-0 flex flex-col gap-4">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<SettingsLabel
				:title="formatMessage(messages.title)"
				:description="
					formatMessage(view.stage === 'connected' ? messages.connectedIntro : messages.intro)
				"
			/>
			<Badge
				v-if="status && view.stage === 'connected'"
				:type="formatMessage(view.broken ? messages.stateBroken : messages.stateConnected)"
				:color="view.broken ? 'red' : 'green'"
			/>
		</div>

		<LoadingIndicator v-if="status === null && loading" />

		<Admonition
			v-else-if="status === null"
			type="critical"
			:header="formatMessage(messages.loadFailed)"
			:body="loadFailure ?? formatMessage(messages.unknownError)"
		>
			<template #actions>
				<Button @click="load()">
					<UpdatedIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<template v-else>
			<p v-if="view.stage === 'unavailable'" class="m-0 text-secondary">
				{{ formatMessage(messages.notSetUp) }}
			</p>

			<template v-else>
				<Admonition
					v-if="externalOff"
					type="warning"
					:header="formatMessage(messages.externalOffHeader)"
					:body="formatMessage(messages.externalOffBody)"
				/>

				<Admonition
					v-if="view.broken"
					type="critical"
					:header="formatMessage(messages.brokenHeader)"
					:body="status.last_error ?? formatMessage(messages.brokenBody)"
				/>

				<Admonition v-if="actionFailure" type="critical" :body="actionFailure" />

				<Admonition
					v-if="view.lastFailure && view.stage === 'unconnected'"
					type="warning"
					:header="formatMessage(messages.lastFailureHeader)"
					:body="view.lastFailure"
				/>

				<template v-if="view.stage === 'unconnected' || view.stage === 'linking'">
					<ul class="m-0 flex list-disc flex-col gap-1 pl-5 text-secondary">
						<li>{{ formatMessage(messages.pointYours) }}</li>
						<li>{{ formatMessage(messages.pointOnce) }}</li>
						<li>{{ formatMessage(messages.pointVisible, { folder: status.folder_name }) }}</li>
						<li>{{ formatMessage(messages.pointUnencrypted) }}</li>
					</ul>

					<div class="flex flex-wrap items-center gap-3">
						<Button type="colored" color="brand" :disabled="busy" @click="beginLink">
							<PlugIcon aria-hidden="true" />
							{{ formatMessage(messages.connectButton) }}
						</Button>
						<span class="text-sm text-secondary">{{ formatMessage(messages.connectFree) }}</span>
					</div>
				</template>

				<template v-else>
					<div class="grid gap-4 sm:grid-cols-2">
						<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
							<span class="text-sm font-semibold text-secondary">
								{{ formatMessage(messages.factAccount) }}
							</span>
							<span class="text-lg font-extrabold text-contrast">
								{{ status.google_name ?? formatMessage(messages.factAccountUnknown) }}
							</span>
							<span v-if="status.google_email" class="break-all text-xs text-secondary">
								{{ status.google_email }}
							</span>
						</div>

						<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
							<span class="text-sm font-semibold text-secondary">
								{{ formatMessage(messages.factStorage) }}
							</span>
							<span class="text-lg font-extrabold text-contrast">
								{{
									view.storage.limitBytes === null
										? formatMessage(messages.factStorageUnlimited)
										: formatMessage(messages.factStorageValue, {
												used: bytes(view.storage.usedBytes ?? 0),
												limit: bytes(view.storage.limitBytes),
											})
								}}
							</span>
							<ProgressBar
								v-if="view.storage.share !== null"
								:progress="Math.round(view.storage.share * 100)"
								:max="100"
								:color="view.storage.nearlyFull ? 'red' : 'brand'"
								full-width
							/>
							<span class="text-xs text-secondary">
								{{
									view.storage.freeBytes === null
										? formatMessage(messages.factStorageWorkspace)
										: formatMessage(messages.factStorageFree, {
												free: bytes(view.storage.freeBytes),
											})
								}}
							</span>
						</div>
					</div>

					<div class="flex flex-wrap items-center gap-x-6 gap-y-1 text-sm text-secondary">
						<span>
							{{ formatMessage(messages.factFolder) }}
							<span class="font-mono text-contrast">{{ status.folder_name }}</span>
						</span>
						<span>
							{{
								status.checked_at
									? formatMessage(messages.checkedAt, { ago: relativeTime(status.checked_at) })
									: formatMessage(messages.checkedNever)
							}}
						</span>
					</div>

					<div class="flex flex-wrap gap-2">
						<Button :disabled="busy" @click="check">
							<UpdatedIcon aria-hidden="true" />
							{{ formatMessage(messages.check) }}
						</Button>
						<Button type="colored" color="red" :disabled="busy" @click="disconnectModal?.show()">
							<UnlinkIcon aria-hidden="true" />
							{{ formatMessage(messages.disconnect) }}
						</Button>
					</div>
				</template>
			</template>
		</template>

		<NewModal
			ref="linkModal"
			:header="formatMessage(messages.linkTitle)"
			width="34rem"
			:on-hide="endLink"
		>
			<div v-if="link" class="flex flex-col gap-4">
				<p class="m-0 text-secondary">{{ formatMessage(messages.linkIntro) }}</p>

				<div class="flex flex-col gap-2">
					<a
						class="break-all text-lg font-semibold text-link"
						:href="link.verification_url"
						target="_blank"
						rel="noopener noreferrer"
					>
						{{ link.verification_url }}
						<ExternalIcon aria-hidden="true" class="inline size-4" />
					</a>
				</div>

				<div class="flex flex-col gap-2">
					<span class="text-sm font-semibold text-secondary">
						{{ formatMessage(messages.linkCode) }}
					</span>
					<span class="select-all font-mono text-3xl font-extrabold tracking-widest text-contrast">
						{{ readableCode(link) }}
					</span>
					<CopyCode :text="readableCode(link)" />
				</div>

				<div v-if="phase === 'waiting'" class="flex items-center gap-3 text-secondary">
					<ProgressSpinner :progress="countdown.progress" class="size-6 text-brand" />
					<span>{{ formatMessage(messages.linkWaiting, { remaining: countdown.remaining }) }}</span>
				</div>

				<Admonition
					v-else-if="phase === 'accepted'"
					type="success"
					:body="formatMessage(messages.linkAccepted)"
				/>
				<Admonition
					v-else-if="phase === 'denied'"
					type="critical"
					:body="view.lastFailure ?? formatMessage(messages.linkDenied)"
				/>
				<Admonition
					v-else
					type="warning"
					:body="view.lastFailure ?? formatMessage(messages.linkExpired)"
				/>

				<Admonition v-if="linkFailure" type="warning" :body="linkFailure" />

				<div class="mb-1 mt-2 flex justify-end gap-2.5">
					<Button v-if="phase === 'waiting'" :disabled="busy" @click="linkModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<template v-else-if="phase !== 'accepted'">
						<Button :disabled="busy" @click="linkModal?.hide()">
							{{ formatMessage(commonMessages.cancelButton) }}
						</Button>
						<Button type="colored" color="brand" :disabled="busy" @click="restartLink">
							<UpdatedIcon aria-hidden="true" />
							{{ formatMessage(messages.linkRetry) }}
						</Button>
					</template>
				</div>
			</div>
		</NewModal>

		<NewModal ref="disconnectModal" :header="formatMessage(messages.disconnect)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">{{ formatMessage(messages.disconnectBody) }}</p>
				<p class="m-0 text-secondary">{{ formatMessage(messages.disconnectFiles) }}</p>
				<Admonition v-if="disconnectFailure" type="critical" :body="disconnectFailure" />

				<div class="mb-1 mt-2 flex flex-wrap justify-end gap-2.5">
					<Button :disabled="busy" @click="disconnectModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button :disabled="busy" @click="disconnect('keep')">
						{{ formatMessage(messages.disconnectKeep) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="disconnect('delete')">
						<UnlinkIcon aria-hidden="true" />
						{{ formatMessage(messages.disconnectDelete) }}
					</Button>
				</div>
			</div>
		</NewModal>
	</Card>
</template>

<script setup lang="ts">
import { ExternalIcon, PlugIcon, UnlinkIcon, UpdatedIcon, XIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	Card,
	commonMessages,
	CopyCode,
	defineMessages,
	LoadingIndicator,
	NewModal,
	ProgressBar,
	ProgressSpinner,
	SettingsLabel,
	useFormatBytes,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { useNow } from '@vueuse/core'
import { computed, onUnmounted, ref } from 'vue'

import { isApiRequestError } from '@/api'
import {
	drive,
	type DriveFileDisposal,
	type DriveLink,
	type DriveStatus,
	linkPhase,
	statusPollMs,
} from '@/api/drive'
import { linkCountdown, driveView, readableCode } from './drive'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const relativeTime = useRelativeTime()
const now = useNow({ interval: 1000 })

const messages = defineMessages({
	title: { id: 'account.drive.title', defaultMessage: 'Backups in your own Google Drive' },
	intro: {
		id: 'account.drive.intro',
		defaultMessage:
			'Connect your Google account and the backups of your servers go into your own Drive instead of onto this machine. The panel only ever sees the folder it made itself.',
	},
	connectedIntro: {
		id: 'account.drive.connected-intro',
		defaultMessage:
			'Your backups go into your own Drive. You can open, download and delete them there yourself.',
	},
	stateConnected: { id: 'account.drive.state.connected', defaultMessage: 'Connected' },
	stateBroken: { id: 'account.drive.state.broken', defaultMessage: 'Connection lost' },
	loadFailed: { id: 'account.drive.load-failed', defaultMessage: 'Could not load the Drive status' },
	unknownError: {
		id: 'account.drive.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	notSetUp: {
		id: 'account.drive.not-set-up',
		defaultMessage:
			'The operator of this panel has not set up Google Drive, so backups stay on this machine.',
	},
	externalOffHeader: {
		id: 'account.drive.external-off.header',
		defaultMessage: 'Outbound services are switched off',
	},
	externalOffBody: {
		id: 'account.drive.external-off.body',
		defaultMessage:
			'Google Drive is an outbound service. While an administrator keeps that switch off, nothing is uploaded and no connection can be made.',
	},
	brokenHeader: {
		id: 'account.drive.broken.header',
		defaultMessage: 'Google no longer accepts this connection',
	},
	lastFailureHeader: {
		id: 'account.drive.last-failure.header',
		defaultMessage: 'The last attempt to connect did not work',
	},
	brokenBody: {
		id: 'account.drive.broken.body',
		defaultMessage:
			'No backup can go into your Drive until you connect again. Backups that are already there stay where they are.',
	},
	pointYours: {
		id: 'account.drive.point.yours',
		defaultMessage: 'The archives use your storage, not the operator’s disk.',
	},
	pointOnce: {
		id: 'account.drive.point.once',
		defaultMessage:
			'You confirm once with a code on any device. No password is typed into this panel.',
	},
	pointVisible: {
		id: 'account.drive.point.visible',
		defaultMessage:
			'They land in a folder called “{folder}” that you can see, open and empty yourself.',
	},
	pointUnencrypted: {
		id: 'account.drive.point.unencrypted',
		defaultMessage:
			'A backup holds your whole server directory, and it is not encrypted: anything in your configuration files is readable to whoever can read your Drive.',
	},
	connectButton: { id: 'account.drive.connect', defaultMessage: 'Connect Google Drive' },
	connectFree: {
		id: 'account.drive.connect-free',
		defaultMessage: 'Free, and it can be undone here at any time.',
	},
	factAccount: { id: 'account.drive.fact.account', defaultMessage: 'Google account' },
	factAccountUnknown: {
		id: 'account.drive.fact.account-unknown',
		defaultMessage: 'Not known yet',
	},
	factStorage: { id: 'account.drive.fact.storage', defaultMessage: 'Drive storage' },
	factStorageValue: { id: 'account.drive.fact.storage-value', defaultMessage: '{used} of {limit}' },
	factStorageUnlimited: {
		id: 'account.drive.fact.storage-unlimited',
		defaultMessage: 'No limit',
	},
	factStorageFree: { id: 'account.drive.fact.storage-free', defaultMessage: '{free} still free.' },
	factStorageWorkspace: {
		id: 'account.drive.fact.storage-workspace',
		defaultMessage: 'This account reports no storage limit.',
	},
	factFolder: { id: 'account.drive.fact.folder', defaultMessage: 'Folder' },
	checkedAt: { id: 'account.drive.checked-at', defaultMessage: 'Last confirmed {ago}' },
	checkedNever: { id: 'account.drive.checked-never', defaultMessage: 'Not confirmed yet' },
	check: { id: 'account.drive.check', defaultMessage: 'Check now' },
	disconnect: { id: 'account.drive.disconnect', defaultMessage: 'Disconnect' },
	disconnectBody: {
		id: 'account.drive.disconnect.body',
		defaultMessage:
			'The panel gives its access back to Google and forgets it. New backups then stay on this machine, if the operator allows that.',
	},
	disconnectFiles: {
		id: 'account.drive.disconnect.files',
		defaultMessage:
			'Backups that already lie in your Drive can be deleted along with it, or left where they are — they stay yours either way, and the panel will find them again if you connect this account later.',
	},
	disconnectKeep: { id: 'account.drive.disconnect.keep', defaultMessage: 'Keep the backups' },
	disconnectDelete: {
		id: 'account.drive.disconnect.delete',
		defaultMessage: 'Delete them and disconnect',
	},
	linkTitle: { id: 'account.drive.link.title', defaultMessage: 'Connect Google Drive' },
	linkIntro: {
		id: 'account.drive.link.intro',
		defaultMessage:
			'Open this page on any device, sign in to Google and type the code below. This page notices on its own.',
	},
	linkCode: { id: 'account.drive.link.code', defaultMessage: 'Your code' },
	linkWaiting: {
		id: 'account.drive.link.waiting',
		defaultMessage: 'Waiting for the confirmation at Google… {remaining} left',
	},
	linkAccepted: {
		id: 'account.drive.link.accepted',
		defaultMessage: 'Confirmed. Your backups can go into your Drive from now on.',
	},
	linkDenied: {
		id: 'account.drive.link.denied',
		defaultMessage: 'The request was declined at Google.',
	},
	linkExpired: {
		id: 'account.drive.link.expired',
		defaultMessage: 'The code ran out before it was confirmed, so the panel stopped asking.',
	},
	linkRetry: { id: 'account.drive.link.retry', defaultMessage: 'New code' },
})

const status = ref<DriveStatus | null>(null)
const link = ref<DriveLink | null>(null)
const loading = ref(true)
const busy = ref(false)
const externalOff = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const linkFailure = ref<string | null>(null)
const disconnectFailure = ref<string | null>(null)
const linkModal = ref<InstanceType<typeof NewModal> | null>(null)
const disconnectModal = ref<InstanceType<typeof NewModal> | null>(null)

let statusTimer: ReturnType<typeof setTimeout> | undefined
let linkTimer: ReturnType<typeof setTimeout> | undefined
let resumable = true

const view = computed(() => driveView(status.value))
const phase = computed(() => (link.value ? linkPhase(link.value, now.value.getTime()) : null))
const countdown = computed(() =>
	link.value ? linkCountdown(link.value, now.value.getTime()) : { remaining: '0:00', progress: 0 },
)

function bytes(value: number): string {
	return formatBytes(value)
}

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function note(error: unknown): void {
	if (isApiRequestError(error) && error.code === 'external_services_disabled') {
		externalOff.value = true
		actionFailure.value = null
		return
	}
	actionFailure.value = reason(error)
}

async function load(): Promise<void> {
	loading.value = true
	loadFailure.value = null
	try {
		adopt(await drive.status())
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
	}
}

function adopt(next: DriveStatus): void {
	status.value = next
	loadFailure.value = null
	clearTimeout(statusTimer)
	const wait = statusPollMs(next)
	if (wait !== null) statusTimer = setTimeout(() => void refresh(), wait)

	const running = next.link
	if (resumable && running !== null && link.value === null && linkPhase(running) === 'waiting') {
		link.value = running
		linkModal.value?.show()
		pollLink()
	}
}

async function refresh(): Promise<void> {
	try {
		adopt(await drive.status())
	} catch (error) {
		loadFailure.value = reason(error)
		clearTimeout(statusTimer)
		statusTimer = setTimeout(() => void refresh(), 30_000)
	}
}

void load()

onUnmounted(() => {
	clearTimeout(statusTimer)
	clearTimeout(linkTimer)
})

async function beginLink(): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	linkFailure.value = null
	try {
		link.value = await drive.startLink()
		externalOff.value = false
		resumable = true
		linkModal.value?.show()
		pollLink()
	} catch (error) {
		if (isApiRequestError(error) && error.code === 'drive_already_linked') await refresh()
		else note(error)
	} finally {
		busy.value = false
	}
}

async function restartLink(): Promise<void> {
	await endLink()
	await beginLink()
}

function pollLink(): void {
	clearTimeout(linkTimer)
	const wait = Math.max(1, link.value?.interval ?? 5) * 1000
	linkTimer = setTimeout(() => void turnOfTheLink(), wait)
}

async function turnOfTheLink(): Promise<void> {
	if (link.value === null) return
	try {
		link.value = await drive.link()
		if (linkPhase(link.value, now.value.getTime()) === 'waiting') {
			pollLink()
			return
		}
	} catch {
		link.value = { ...link.value, state: 'accepted' }
	}
	await refresh()
	if (status.value?.configured !== true && link.value.state === 'accepted') {
		link.value = { ...link.value, state: 'expired' }
	}
}

async function endLink(): Promise<void> {
	clearTimeout(linkTimer)
	resumable = false
	const running = link.value
	link.value = null
	if (running === null || linkPhase(running, now.value.getTime()) !== 'waiting') return
	try {
		await drive.cancelLink()
	} catch {
	}
	await refresh()
}

async function check(): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	try {
		adopt(await drive.check())
		externalOff.value = false
	} catch (error) {
		note(error)
	} finally {
		busy.value = false
	}
}

async function disconnect(files: DriveFileDisposal): Promise<void> {
	if (busy.value) return
	busy.value = true
	disconnectFailure.value = null
	try {
		await drive.disconnect(files)
		disconnectModal.value?.hide()
		await refresh()
	} catch (error) {
		disconnectFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}
</script>
