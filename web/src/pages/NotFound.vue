<template>
	<EmptyState
		type="no-search-result"
		:heading="formatMessage(messages.heading)"
		:description="formatMessage(messages.description)"
	>
		<template #actions>
			<ButtonLink :to="{ name: 'servers' }" type="colored" color="brand">
				<LeftArrowIcon />
				{{ formatMessage(messages.back) }}
			</ButtonLink>
		</template>
	</EmptyState>
</template>

<script setup lang="ts">
import { LeftArrowIcon } from '@modrinth/assets'
import { ButtonLink, defineMessages, EmptyState, useVIntl } from '@modrinth/ui'

const { formatMessage } = useVIntl()

const messages = defineMessages({
	heading: {
		id: 'panel.not-found.heading',
		defaultMessage: 'Page not found',
	},
	description: {
		id: 'panel.not-found.description',
		defaultMessage: 'This address does not lead anywhere.',
	},
	back: {
		id: 'panel.not-found.back',
		defaultMessage: 'Back to servers',
	},
})
</script>
