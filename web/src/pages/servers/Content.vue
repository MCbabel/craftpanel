<template>
	<div class="flex flex-col gap-4">
		<Admonition
			v-if="content.truncated.value"
			type="warning"
			:header="formatMessage(messages.truncatedHeader)"
		>
			{{ formatMessage(messages.truncatedBody, { limit: CONTENT_LIMIT }) }}
		</Admonition>

		<Admonition
			v-if="restartHintVisible"
			type="info"
			:header="formatMessage(messages.restartHeader)"
		>
			{{ formatMessage(messages.restartBody) }}
		</Admonition>

		<LoadingIndicator v-if="content.context.loading.value" />

		<ContentCardLayout>
			<template #modals>
				<ContentUpdaterModal
					v-if="updater.visible.value"
					ref="updaterModal"
					:versions="updater.versions.value"
					:current-game-version="content.currentGameVersion.value"
					:current-loader="content.currentLoader.value"
					:current-version-id="updater.currentVersionId.value"
					:is-app="false"
					:project-type="updater.projectType.value"
					:project-name="updater.projectName.value"
					:project-icon-url="updater.projectIconUrl.value"
					:loading="updater.loading.value"
					:loading-changelog="updater.loadingChangelog.value"
					:action-disabled="content.context.isBusy.value"
					:action-disabled-tooltip="busyMessage ?? undefined"
					:warning="updateWarning"
					@update="updater.confirm"
					@cancel="updater.cancel"
					@version-select="updater.select"
					@version-hover="updater.hover"
				/>

				<ModpackContentModal
					ref="modpackContentModal"
					:modpack-name="modpack?.project.title"
					:modpack-icon-url="modpack?.project.icon_url ?? undefined"
					:enable-toggle="false"
				/>

				<UnknownFileWarningModal
					ref="unknownFileModal"
					mode="mod"
					:file-name="unknownFileName"
					@cancel="settleUnknownFile(false)"
					@continue="acceptUnknownFile"
				/>
			</template>
		</ContentCardLayout>
	</div>
</template>

<script setup lang="ts">
import type { UploadState } from '@modrinth/api-client'
import {
	Admonition,
	ContentCardLayout,
	ContentUpdaterModal,
	defineMessages,
	injectModrinthClient,
	injectNotificationManager,
	LoadingIndicator,
	ModpackContentModal,
	UnknownFileWarningModal,
	useVIntl,
} from '@modrinth/ui'
import { computed, nextTick, onScopeDispose, ref, watch } from 'vue'
import { useRouter } from 'vue-router'

import type { UploadProgress } from '@/api'
import { useServerPage } from '@/composables/server-page'
import { useContentManager } from '@/providers/content-manager'

import { updatesNeedRestart } from './restart-hint'

const CONTENT_LIMIT = 2000
const SKIP_UNKNOWN_FILE_WARNING = 'craftpanel.content.skip-unknown-file-warning'

const messages = defineMessages({
	truncatedHeader: {
		id: 'craftpanel.content.truncated-header',
		defaultMessage: 'Not all content is shown',
	},
	truncatedBody: {
		id: 'craftpanel.content.truncated-body',
		defaultMessage:
			'This server has more than {limit, number} files. Filters, counts and bulk actions only cover the ones listed here.',
	},
	restartHeader: {
		id: 'craftpanel.content.restart-header',
		defaultMessage: 'Updates need a restart',
	},
	restartBody: {
		id: 'craftpanel.content.restart-body',
		defaultMessage:
			'The server is running. Installing or updating content replaces the files right away, but the running server keeps the old ones until you restart it.',
	},
	updateWarning: {
		id: 'craftpanel.content.update-warning',
		defaultMessage:
			'Updating can break your world. Review the changelog and back up first. The server is running, so the new version only takes effect after a restart.',
	},
})

const { formatMessage } = useVIntl()
const router = useRouter()
const page = useServerPage()
const client = injectModrinthClient()
const { addNotification } = injectNotificationManager()

const updaterModal = ref<InstanceType<typeof ContentUpdaterModal> | null>(null)
const modpackContentModal = ref<InstanceType<typeof ModpackContentModal> | null>(null)
const unknownFileModal = ref<InstanceType<typeof UnknownFileWarningModal> | null>(null)

const content = useContentManager({
	serverId: page.serverId,
	socket: page.socket,
	busyReasons: page.context.busyReasons,
	notify: addNotification,
	browse: browseCatalogue,
	modrinth: client.labrinth.versions_v2,
	fileLink: (path, editing) => ({
		name: 'server-files',
		params: { id: page.serverId },
		query: editing === undefined ? { path } : { path, editing },
	}),
	openSettings: () =>
		void router.push({
			name: 'server-settings',
			params: { id: page.serverId, section: 'installation' },
		}),
	updaterModal,
	modpackContentModal,
	confirmUnknownFile,
})

const updater = content.updater
const modpack = computed(() => content.context.modpack.value)
const busyMessage = computed(() => content.context.busyMessage?.value)

const isRunning = computed(() => page.context.powerState.value === 'running')
const restartHintVisible = computed(() =>
	updatesNeedRestart(
		isRunning.value,
		content.context.items.value,
		modpack.value?.hasUpdate === true,
	),
)
const updateWarning = computed(() =>
	isRunning.value ? formatMessage(messages.updateWarning) : undefined,
)

function toUploadState(progress: UploadProgress | null): UploadState {
	return {
		isUploading: progress !== null,
		currentFileName: null,
		currentFileProgress: progress?.progress ?? 0,
		uploadedBytes: progress?.loaded ?? 0,
		totalBytes: progress?.total ?? 0,
		completedFiles: 0,
		totalFiles: progress ? 1 : 0,
	}
}

watch(
	() => content.uploadProgress.value,
	(progress) => {
		page.context.uploadState.value = toUploadState(progress)
		page.context.cancelUpload.value = progress ? content.cancelUpload : null
	},
)

onScopeDispose(() => {
	if (content.uploadProgress.value === null) return
	page.context.uploadState.value = toUploadState(null)
	page.context.cancelUpload.value = null
})

function browseCatalogue(): void {
	void router.push({ name: 'server-browse', params: { id: page.serverId } })
}

const unknownFileName = ref('')
let skipUnknownFileWarning = localStorage.getItem(SKIP_UNKNOWN_FILE_WARNING) === 'true'
let settleUnknown: ((confirmed: boolean) => void) | null = null

function confirmUnknownFile(fileName: string): Promise<boolean> {
	if (skipUnknownFileWarning) return Promise.resolve(true)
	unknownFileName.value = fileName
	return new Promise((resolve) => {
		settleUnknown = resolve
		void nextTick(() => unknownFileModal.value?.show())
	})
}

function settleUnknownFile(confirmed: boolean): void {
	const resolve = settleUnknown
	settleUnknown = null
	unknownFileName.value = ''
	resolve?.(confirmed)
}

function acceptUnknownFile(dontShowAgain: boolean): void {
	if (dontShowAgain) {
		skipUnknownFileWarning = true
		localStorage.setItem(SKIP_UNKNOWN_FILE_WARNING, 'true')
	}
	settleUnknownFile(true)
}
</script>

<style scoped>
:deep(.flex.flex-wrap.items-center.justify-between > .flex.items-center.gap-2) {
	flex-wrap: wrap;
}
</style>
