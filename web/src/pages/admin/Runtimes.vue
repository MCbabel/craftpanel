<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<LoadingIndicator v-if="overview === null && loading" />

		<Admonition
			v-else-if="overview === null"
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
			<Admonition
				v-if="actionFailure"
				type="critical"
				:body="actionFailure"
				dismissible
				@dismiss="actionFailure = null"
			/>

			<Card class="!mb-0 flex flex-col gap-4">
				<div class="flex flex-wrap items-center gap-3">
					<Badge
						:type="
							formatMessage(overview.auto_install ? messages.autoOn : messages.autoOff)
						"
						:color="overview.auto_install ? 'green' : 'orange'"
					/>
					<span class="text-sm text-secondary">
						{{
							formatMessage(
								overview.auto_install ? messages.autoOnHint : messages.autoOffHint,
							)
						}}
					</span>
					<RouterLink
						class="text-sm text-link hover:underline"
						:to="{ name: 'admin-settings' }"
					>
						{{ formatMessage(messages.autoSwitch) }}
					</RouterLink>
				</div>

				<dl class="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
					<dt class="text-secondary">{{ formatMessage(messages.directory) }}</dt>
					<dd class="m-0 break-all font-mono text-contrast">{{ overview.directory }}</dd>
					<dt class="text-secondary">{{ formatMessage(messages.spaceUsed) }}</dt>
					<dd class="m-0 text-contrast">{{ formatBytes(overview.total_bytes) }}</dd>
					<dt class="text-secondary">{{ formatMessage(messages.machine) }}</dt>
					<dd class="m-0 text-contrast">
						{{ overview.architecture ?? formatMessage(messages.machineUnknown) }}
					</dd>
				</dl>

				<Admonition
					v-if="overview.architecture === null"
					type="warning"
					:header="formatMessage(messages.unsupportedHeader)"
					:body="formatMessage(messages.unsupportedBody)"
				/>
			</Card>

			<Card class="!mb-0 flex flex-col gap-6">
				<div
					v-for="(entry, index) of overview.majors"
					:key="entry.major"
					class="flex flex-col gap-3"
					:class="index > 0 ? 'border-0 border-t border-solid border-divider pt-6' : ''"
				>
					<div class="flex flex-wrap items-start justify-between gap-3">
						<div class="flex min-w-0 flex-col gap-1">
							<div class="flex flex-wrap items-center gap-2">
								<span class="text-lg font-extrabold text-contrast">
									{{ formatMessage(messages.javaMajor, { major: entry.major }) }}
								</span>
								<Badge :type="standingLabel(entry)" :color="standingColor(entry)" />
							</div>
							<span class="text-sm text-secondary">{{ coversLabel(entry.major) }}</span>
						</div>

						<div class="flex flex-wrap items-center gap-2">
							<Button
								v-if="entry.fetchable"
								:disabled="!canFetch(overview, entry)"
								@click="fetchOne(entry)"
							>
								<UpdatedIcon v-if="entry.runtime" aria-hidden="true" />
								<DownloadIcon v-else aria-hidden="true" />
								{{
									formatMessage(entry.runtime ? messages.fetchAgain : messages.install)
								}}
							</Button>
							<Button
								v-if="entry.runtime"
								type="colored"
								color="red"
								:disabled="!canRemove(entry)"
								@click="askToRemove(entry)"
							>
								<TrashIcon aria-hidden="true" />
								{{ formatMessage(messages.remove) }}
							</Button>
						</div>
					</div>

					<dl
						v-if="entry.runtime"
						class="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm"
					>
						<dt class="text-secondary">{{ formatMessage(messages.version) }}</dt>
						<dd class="m-0 text-contrast">
							{{ entry.runtime.version }} · {{ vendorLabel(entry.runtime.vendor) }}
						</dd>
						<dt class="text-secondary">{{ formatMessage(messages.size) }}</dt>
						<dd class="m-0 text-contrast">{{ formatBytes(entry.runtime.size_bytes) }}</dd>
						<dt class="text-secondary">{{ formatMessage(messages.laidAt) }}</dt>
						<dd class="m-0 text-contrast">
							{{
								entry.runtime.laid_at
									? relativeTime(entry.runtime.laid_at)
									: formatMessage(messages.laidUnknown)
							}}
						</dd>
						<dt class="text-secondary">{{ formatMessage(messages.where) }}</dt>
						<dd class="m-0 break-all font-mono text-xs text-contrast">
							{{ entry.runtime.directory }}
						</dd>
					</dl>

					<p v-else-if="entry.system" class="m-0 text-sm text-secondary">
						{{
							formatMessage(messages.systemFound, {
								version: entry.system.version,
								vendor: vendorLabel(entry.system.vendor),
								path: entry.system.path,
							})
						}}
					</p>

					<p v-else class="m-0 text-sm text-secondary">
						{{
							formatMessage(
								entry.fetchable ? messages.absentFetchable : messages.absentForeign,
							)
						}}
					</p>

					<p class="m-0 text-sm text-secondary">
						{{ formatMessage(messages.servers, { count: entry.servers }) }}
						<span v-if="entry.running.length > 0" class="font-semibold text-contrast">
							{{
								formatMessage(messages.runningNow, { names: entry.running.join(', ') })
							}}
						</span>
					</p>

					<Admonition
						v-if="entry.running.length > 0 && entry.runtime"
						type="info"
						:body="formatMessage(messages.lockedBody)"
					/>

					<div v-if="busy(entry)" class="flex flex-col gap-1">
						<span class="text-sm font-medium text-secondary">
							{{ stageLabel(entry) }}
						</span>
						<ProgressBar
							:progress="entry.job?.share ?? 0"
							:waiting="(entry.job?.share ?? 0) === 0"
							full-width
						/>
					</div>

					<Admonition
						v-else-if="failureOf(entry)"
						type="critical"
						:header="formatMessage(messages.attemptFailed)"
						:body="failureOf(entry) ?? ''"
					/>
				</div>
			</Card>
		</template>

		<NewModal ref="removeModal" :header="formatMessage(messages.remove)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">
					{{ formatMessage(messages.removeBody, { major: removing?.major ?? 0 }) }}
				</p>
				<Admonition type="warning" :body="formatMessage(messages.removeWarning)" />
				<Admonition v-if="actionFailure" type="critical" :body="actionFailure" />

				<div class="mb-1 mt-2 flex justify-end gap-2.5">
					<Button :disabled="working" @click="removeModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button type="colored" color="red" :disabled="working" @click="removeOne">
						<TrashIcon aria-hidden="true" />
						{{ formatMessage(messages.remove) }}
					</Button>
				</div>
			</div>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import { DownloadIcon, TrashIcon, UpdatedIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	Card,
	commonMessages,
	defineMessages,
	LoadingIndicator,
	NewModal,
	ProgressBar,
	useFormatBytes,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { onMounted, onUnmounted, ref } from 'vue'
import { RouterLink } from 'vue-router'

import {
	api,
	isApiRequestError,
	type JavaMajorEntry,
	type JavaRuntimeOverview,
	type JreVendor,
} from '@/api'

import { busy, canFetch, canRemove, failureOf, pollDelay, standingOf } from './runtimes'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const relativeTime = useRelativeTime()

const VENDOR_LABELS: Record<JreVendor, string> = {
	corretto: 'Corretto',
	temurin: 'Temurin',
	graal: 'GraalVM',
}

const messages = defineMessages({
	title: { id: 'admin.runtimes.title', defaultMessage: 'Java runtimes' },
	subtitle: {
		id: 'admin.runtimes.subtitle',
		defaultMessage:
			'Every Minecraft version needs a particular Java. These are the ones this panel keeps for your servers, and the ones it can fetch from Adoptium.',
	},
	loadFailed: { id: 'admin.runtimes.load-failed', defaultMessage: 'Could not read the runtimes' },
	unknownError: {
		id: 'admin.runtimes.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	autoOn: { id: 'admin.runtimes.auto-on', defaultMessage: 'Fetching by itself' },
	autoOff: { id: 'admin.runtimes.auto-off', defaultMessage: 'Fetching switched off' },
	autoOnHint: {
		id: 'admin.runtimes.auto-on-hint',
		defaultMessage: 'A server that needs a Java that is missing gets it while it starts.',
	},
	autoOffHint: {
		id: 'admin.runtimes.auto-off-hint',
		defaultMessage:
			'A server that needs a Java that is missing refuses to start. The buttons here still work: you asked for it.',
	},
	autoSwitch: { id: 'admin.runtimes.auto-switch', defaultMessage: 'Change in panel settings' },
	directory: { id: 'admin.runtimes.directory', defaultMessage: 'Kept in' },
	spaceUsed: { id: 'admin.runtimes.space-used', defaultMessage: 'Disk used' },
	machine: { id: 'admin.runtimes.machine', defaultMessage: 'This machine' },
	machineUnknown: {
		id: 'admin.runtimes.machine-unknown',
		defaultMessage: 'not one Adoptium builds for',
	},
	unsupportedHeader: {
		id: 'admin.runtimes.unsupported-header',
		defaultMessage: 'Nothing can be fetched on this machine',
	},
	unsupportedBody: {
		id: 'admin.runtimes.unsupported-body',
		defaultMessage:
			'Adoptium builds its Linux runtimes for x64 and aarch64, and this is neither. Install Java with the package manager; the panel finds it and lists it below.',
	},
	javaMajor: { id: 'admin.runtimes.java-major', defaultMessage: 'Java {major}' },
	covers8: { id: 'admin.runtimes.covers-8', defaultMessage: 'For Minecraft 1.16 and older' },
	covers17: { id: 'admin.runtimes.covers-17', defaultMessage: 'For Minecraft 1.17 to 1.19' },
	covers21: { id: 'admin.runtimes.covers-21', defaultMessage: 'For Minecraft 1.20 to 1.25' },
	covers25: { id: 'admin.runtimes.covers-25', defaultMessage: 'For Minecraft 1.26 and newer' },
	coversOther: {
		id: 'admin.runtimes.covers-other',
		defaultMessage: 'The panel asks for this one for no version; somebody put it here',
	},
	standingLaid: { id: 'admin.runtimes.standing-laid', defaultMessage: 'Fetched by the panel' },
	standingSystem: { id: 'admin.runtimes.standing-system', defaultMessage: 'From the system' },
	standingAbsent: { id: 'admin.runtimes.standing-absent', defaultMessage: 'Not here' },
	version: { id: 'admin.runtimes.version', defaultMessage: 'Version' },
	size: { id: 'admin.runtimes.size', defaultMessage: 'Size' },
	laidAt: { id: 'admin.runtimes.laid-at', defaultMessage: 'Fetched' },
	laidUnknown: { id: 'admin.runtimes.laid-unknown', defaultMessage: 'at some point' },
	where: { id: 'admin.runtimes.where', defaultMessage: 'Directory' },
	systemFound: {
		id: 'admin.runtimes.system-found',
		defaultMessage:
			'The panel has fetched none. This machine already carries {version} ({vendor}) at {path}, and servers use that one.',
	},
	absentFetchable: {
		id: 'admin.runtimes.absent-fetchable',
		defaultMessage: 'Not on this machine. A server that needs it cannot start until it is here.',
	},
	absentForeign: {
		id: 'admin.runtimes.absent-foreign',
		defaultMessage: 'Not on this machine, and this panel does not fetch this version.',
	},
	servers: {
		id: 'admin.runtimes.servers',
		defaultMessage: '{count, plural, =0 {No server asks for it} one {# server asks for it} other {# servers ask for it}}.',
	},
	runningNow: {
		id: 'admin.runtimes.running-now',
		defaultMessage: 'Running on it right now: {names}',
	},
	lockedBody: {
		id: 'admin.runtimes.locked-body',
		defaultMessage:
			'While a server runs on this runtime it is neither replaced nor removed: the files would be pulled out from under the running Java. Stop the server first.',
	},
	install: { id: 'admin.runtimes.install', defaultMessage: 'Fetch it' },
	fetchAgain: { id: 'admin.runtimes.fetch-again', defaultMessage: 'Fetch again' },
	remove: { id: 'admin.runtimes.remove', defaultMessage: 'Remove' },
	removeBody: {
		id: 'admin.runtimes.remove-body',
		defaultMessage: 'The Java {major} directory the panel fetched is deleted from this machine.',
	},
	removeWarning: {
		id: 'admin.runtimes.remove-warning',
		defaultMessage:
			'A server that needs this version will fetch it again on its next start, as long as fetching is switched on. With fetching off it will not start.',
	},
	attemptFailed: { id: 'admin.runtimes.attempt-failed', defaultMessage: 'The last attempt failed' },
	stageWaiting: { id: 'admin.runtimes.stage-waiting', defaultMessage: 'Queued' },
	stageAsking: { id: 'admin.runtimes.stage-asking', defaultMessage: 'Asking Adoptium' },
	stageDownloading: {
		id: 'admin.runtimes.stage-downloading',
		defaultMessage: 'Downloading {done} of {total}',
	},
	stageUnpacking: { id: 'admin.runtimes.stage-unpacking', defaultMessage: 'Unpacking' },
	stageDone: { id: 'admin.runtimes.stage-done', defaultMessage: 'Finishing up' },
})

const COVERS = {
	8: messages.covers8,
	17: messages.covers17,
	21: messages.covers21,
	25: messages.covers25,
} as const

const overview = ref<JavaRuntimeOverview | null>(null)
const removing = ref<JavaMajorEntry | null>(null)
const loading = ref(true)
const working = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const removeModal = ref<InstanceType<typeof NewModal> | null>(null)

let timer: ReturnType<typeof setTimeout> | undefined

function vendorLabel(vendor: JreVendor): string {
	return VENDOR_LABELS[vendor]
}

function coversLabel(major: number): string {
	const known = COVERS[major as keyof typeof COVERS]
	return formatMessage(known ?? messages.coversOther)
}

function standingLabel(entry: JavaMajorEntry): string {
	const standing = standingOf(entry)
	if (standing === 'laid') return formatMessage(messages.standingLaid)
	if (standing === 'system') return formatMessage(messages.standingSystem)
	return formatMessage(messages.standingAbsent)
}

function standingColor(entry: JavaMajorEntry): string {
	const standing = standingOf(entry)
	if (standing === 'laid') return 'green'
	return standing === 'system' ? 'blue' : 'orange'
}

function stageLabel(entry: JavaMajorEntry): string {
	const job = entry.job
	if (!job) return formatMessage(messages.stageWaiting)
	if (job.stage === 'asking') return formatMessage(messages.stageAsking)
	if (job.stage === 'unpacking') return formatMessage(messages.stageUnpacking)
	if (job.stage === 'done') return formatMessage(messages.stageDone)
	if (job.stage === 'downloading') {
		return formatMessage(messages.stageDownloading, {
			done: formatBytes(job.done_bytes),
			total: formatBytes(job.total_bytes),
		})
	}
	return formatMessage(messages.stageWaiting)
}

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function adopt(seen: JavaRuntimeOverview): void {
	overview.value = seen
	clearTimeout(timer)
	const delay = pollDelay(seen)
	if (delay !== null) timer = setTimeout(() => void load(true), delay)
}

async function load(quiet = false): Promise<void> {
	if (!quiet) loading.value = true
	try {
		adopt(await api.admin.javaRuntimes())
		loadFailure.value = null
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
	}
}

onMounted(() => void load())
onUnmounted(() => clearTimeout(timer))

async function fetchOne(entry: JavaMajorEntry): Promise<void> {
	if (working.value) return
	working.value = true
	actionFailure.value = null
	try {
		adopt(await api.admin.fetchJavaRuntime(entry.major))
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		working.value = false
	}
}

function askToRemove(entry: JavaMajorEntry): void {
	removing.value = entry
	actionFailure.value = null
	removeModal.value?.show()
}

async function removeOne(): Promise<void> {
	const entry = removing.value
	if (entry === null || working.value) return
	working.value = true
	actionFailure.value = null
	try {
		adopt(await api.admin.removeJavaRuntime(entry.major))
		removeModal.value?.hide()
		removing.value = null
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		working.value = false
	}
}
</script>
