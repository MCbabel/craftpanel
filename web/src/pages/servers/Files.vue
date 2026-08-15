<template>
	<LoadingIndicator v-if="files.pending.value" />
	<FilePageLayout :show-refresh-button="true" />
</template>

<script setup lang="ts">
import {
	defineMessages,
	FilePageLayout,
	injectNotificationManager,
	LoadingIndicator,
	useVIntl,
} from '@modrinth/ui'
import { useRoute } from 'vue-router'

import { hasErrorCode, isApiRequestError } from '@/api'
import { useServerPage } from '@/composables/server-page'
import { provideServerFileManager } from '@/providers/file-manager'

import { type FileToOpen, fileToOpen, folderToOpen } from './file-link'

const { formatMessage } = useVIntl()
const { addNotification } = injectNotificationManager()
const page = useServerPage()

const files = provideServerFileManager({
	serverId: page.serverId,
	socket: page.socket,
	permissions: () => page.server.value.current_user_permissions,
	uploadState: page.context.uploadState,
})

const messages = defineMessages({
	openFailed: { id: 'craftpanel.files.open-failed', defaultMessage: 'Could not open {file}' },
	openMissing: {
		id: 'craftpanel.files.open-missing',
		defaultMessage:
			'There is no {file} on this server yet. A server writes it when it starts for the first time.',
	},
})

page.context.cancelUpload.value = files.cancelUpload

async function openFromLink(wanted: FileToOpen): Promise<void> {
	files.context.navigateTo(wanted.directory)
	try {
		await files.context.readFile(wanted.path)
	} catch (failure) {
		addNotification({
			title: formatMessage(messages.openFailed, { file: wanted.name }),
			text: hasErrorCode(failure, 'not_found')
				? formatMessage(messages.openMissing, { file: wanted.name })
				: failure instanceof Error
					? failure.message
					: undefined,
			type: 'error',
			errorCode: isApiRequestError(failure) ? failure.code : undefined,
		})
		return
	}
	files.context.startEditing(wanted)
}

const query = useRoute().query
const wanted = fileToOpen(query)
const folder = folderToOpen(query)
if (wanted !== null) void openFromLink(wanted)
else if (folder !== null) files.context.navigateTo(folder)
</script>
