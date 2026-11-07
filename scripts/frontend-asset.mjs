// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { auditDirectory, webFileAllowed } from './package-audit.mjs'
import { verifyArchive } from './package-artifacts.mjs'
import { publicIdentity, versionPattern } from './release-identity.mjs'
import { json, root, sha256 } from './release-utils.mjs'

export const frontendAudit = {
	allowed: webFileAllowed,
	frontend: true,
	required: [
		'package.json',
		'release.json',
		'LICENSE',
		'THIRD_PARTY_NOTICES.txt',
		'assets/index.html',
	],
}

/** Portable identity of the exact published Web dependency. */
export function frontendInput(record) {
	publicIdentity(record.version)
	return {
		name: '@bluink/web',
		version: record.version,
		integrity: record.integrity,
		sha256: record.sha256,
	}
}

export function validateFrontend(input) {
	if (
		input?.name !== '@bluink/web' ||
		!versionPattern.test(input.version) ||
		!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(input.integrity) ||
		!/^[a-f0-9]{64}$/.test(input.sha256) ||
		JSON.stringify(Object.keys(input).sort()) !==
			JSON.stringify(['integrity', 'name', 'sha256', 'version'])
	)
		throw new Error('Invalid fixed frontend asset identity or version.')
	return input
}

export async function downloadFrontend(input, destination, fetchAsset = fetch) {
	validateFrontend(input)
	await mkdir(destination, { recursive: true })
	const response = await fetchAsset(
		`https://registry.npmjs.org/@bluink/web/-/web-${input.version}.tgz`,
		{
			signal: AbortSignal.timeout(120_000),
		},
	)
	if (!response.ok)
		throw new Error(
			`Publish the exact Web package first: HTTP ${response.status}`,
		)
	const filename = `bluink-web-${input.version}.tgz`
	const archive = path.join(destination, filename)
	await writeFile(archive, Buffer.from(await response.arrayBuffer()))
	return verifyArchive({ ...input, filename, archive }, frontendAudit)
}

/** Stage exact Web bytes for a source checkout's native test executable. */
export async function stageFrontend(record, destination) {
	return verifyArchive(record, {
		...frontendAudit,
		onInspect: async (directory) => {
			await rm(destination, { recursive: true, force: true })
			await cp(directory, destination, { recursive: true })
		},
	})
}

/** Bind the actual bundled files to the approved frontend archive. */
export async function verifyBundledFrontend(directory, record) {
	await verifyArchive(record, {
		...frontendAudit,
		onInspect: async (source, inspected) => {
			const files = await auditDirectory(directory, {
				allowed: webFileAllowed,
				frontend: true,
			})
			if (JSON.stringify(files) !== JSON.stringify(inspected.files))
				throw new Error(
					'Bundled frontend file set differs from the approved asset.',
				)
			for (const file of files)
				if (
					sha256(await readFile(path.join(source, file))) !==
					sha256(await readFile(path.join(directory, file)))
				)
					throw new Error(
						`Bundled frontend differs from the approved asset: ${file}`,
					)
		},
	})
}

/** Source builds consume a verified cache or the exact registry archive. */
export async function stageBuildFrontend(installRoot = root) {
	const input = await json(path.join(installRoot, 'release-input.json'))
	validateFrontend(input.frontend)
	const destination = path.join(installRoot, '.resource-packages')
	const filename = `bluink-web-${input.frontend.version}.tgz`
	const record = {
		...input.frontend,
		filename,
		archive: path.join(destination, filename),
	}
	try {
		await readFile(record.archive)
	} catch (error) {
		if (error.code !== 'ENOENT') throw error
		await downloadFrontend(input.frontend, destination)
	}
	await stageFrontend(record, path.join(installRoot, 'backend/web'))
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	if (process.argv.slice(2).join(' ') !== 'stage')
		throw new Error('Choose stage.')
	await stageBuildFrontend()
}
