// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

export const versionPattern =
	/^(0|[1-9]\d?)\.(1[0-2]|[1-9])\.([1-9]\d*)(?:-(alpha|beta|rc))?$/

export function calendarMonth(date = new Date()) {
	return {
		key: `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, '0')}`,
		prefix: `${date.getUTCFullYear() % 100}.${date.getUTCMonth() + 1}`,
	}
}

export function nextVersion(versions, reserved, channel, date = new Date()) {
	if (!['alpha', 'beta', 'rc', 'stable'].includes(channel))
		throw new Error('Choose alpha, beta, rc or stable.')
	if (!Number.isSafeInteger(reserved) || reserved < 0)
		throw new Error('The release counter is invalid.')
	const { prefix } = calendarMonth(date)
	let build = reserved
	for (const version of versions) {
		const match = versionPattern.exec(version)
		if (match && `${match[1]}.${match[2]}` === prefix)
			build = Math.max(build, Number(match[3]))
	}
	if (!Number.isSafeInteger(build + 1))
		throw new Error('The monthly release counter is exhausted.')
	return `${prefix}.${build + 1}${channel === 'stable' ? '' : `-${channel}`}`
}

/** Only explicit public fields may enter a distributed release manifest. */
export function publicIdentity(version, metadata = {}, official = false) {
	const allowed = new Set([
		'channel',
		'publicCommit',
		'publicRepository',
		'toolchain',
	])
	for (const key of Object.keys(metadata))
		if (!allowed.has(key))
			throw new Error(`Unexpected public release metadata: ${key}`)
	const match = versionPattern.exec(version)
	if (!match && version !== '0.0.0-dev')
		throw new Error('Invalid calendar release version.')
	const channel = match?.[4] ?? (match ? 'stable' : 'alpha')
	if (metadata.channel && metadata.channel !== channel)
		throw new Error('Release channel does not match the version.')
	if (metadata.publicCommit && !/^[a-f0-9]{40}$/.test(metadata.publicCommit))
		throw new Error('Invalid public source commit.')
	if (
		metadata.publicRepository &&
		!/^https:\/\/github\.com\/[\w.-]+\/[\w.-]+$/.test(metadata.publicRepository)
	)
		throw new Error(
			'Use the public GitHub repository URL without credentials or query parameters.',
		)
	if (
		official &&
		(!match || !metadata.publicCommit || !metadata.publicRepository)
	)
		throw new Error(
			'An official release requires a calendar version and public source identity.',
		)
	if (metadata.toolchain) {
		for (const [key, value] of Object.entries(metadata.toolchain))
			if (
				!['node', 'npm', 'rust'].includes(key) ||
				!/^[\w.()+ -]{1,100}$/.test(value)
			)
				throw new Error(`Invalid public toolchain identity: ${key}`)
	}
	return { ...metadata, channel }
}
