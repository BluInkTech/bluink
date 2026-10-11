// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, mkdtemp, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { packDirectory, verifyArchive } from './package-artifacts.mjs'
import { webFileAllowed } from './package-audit.mjs'
import { json, root, writeJson } from './release-utils.mjs'

/** Portable artifact metadata: never transmit runner-specific absolute paths. */
export function resourceSummary({
	name,
	version,
	filename,
	integrity,
	sha256,
}) {
	return { name, version, filename, integrity, sha256 }
}

/** Consume the exact shared archives prepared by the single resource job. */
export async function readPreparedResources(input, directory, installRoot) {
	const entries = await json(path.join(directory, 'dependencies.json'))
	if (
		!Array.isArray(entries) ||
		entries.length !== 3 ||
		new Set(entries.map((entry) => entry.name)).size !== 3
	)
		throw new Error('Incomplete shared resource archive set.')
	const records = []
	const options = new Map()
	for (const entry of entries) {
		const expected = input.dependencies.find(
			(dependency) => dependency.name === entry.name,
		)
		if (
			!expected ||
			entry.filename !==
				`${entry.name.replace('@', '').replace('/', '-')}-${entry.version}.tgz` ||
			Object.keys(entry).some(
				(key) =>
					!['name', 'version', 'filename', 'integrity', 'sha256'].includes(key),
			) ||
			['name', 'version', 'integrity', 'sha256'].some(
				(key) => entry[key] !== expected[key],
			)
		)
			throw new Error(
				'Shared resource differs from frozen input or has unsafe metadata.',
			)
		const kind = entry.name.split('/')[1]
		const pins =
			kind === 'typst'
				? await json(path.join(root, 'typst/sources.json'))
				: undefined
		const audit = {
			allowed:
				kind === 'web'
					? webFileAllowed
					: (file) => resourceFileAllowed(kind, file),
			frontend: kind === 'web',
			trustedUpstream: Object.fromEntries(
				Object.entries(pins?.targets ?? {}).map(([target, spec]) => [
					`assets/${target}/${spec.binary}`,
					spec.binarySha256,
				]),
			),
		}
		const record = { ...entry, archive: path.join(directory, entry.filename) }
		records.push(await verifyArchive(record, audit))
		options.set(entry.name, audit)
	}
	// Verify every frozen archive before changing workspace assets. Workspace
	// links keep fonts/Typst source tests pointed at these exact shared bytes.
	if (installRoot)
		for (const record of records.filter(
			(entry) => entry.name !== '@bluink/web',
		))
			await verifyArchive(record, {
				...options.get(record.name),
				onInspect: async (directory) => {
					const destination = path.join(
						installRoot,
						record.name.split('/')[1],
						'assets',
					)
					await rm(destination, { recursive: true, force: true })
					await cp(path.join(directory, 'assets'), destination, {
						recursive: true,
					})
				},
			})
	return records
}

export function resourceFileAllowed(kind, file) {
	if (
		/^(?:package\.json|index\.(?:js|d\.ts)|sources\.json|LICENSE|README\.md)$/.test(
			file,
		)
	)
		return true
	if (kind === 'fonts')
		return /^(?:catalogue\.(?:json|js|d\.ts)|assets\/(?:manifest\.json|web\/(?:[\w-]+\.css|licenses\.json|licenses\/[\w-]+\.txt|[\w-]+\/[\w-]+\.woff2)|pdf\/[\w-]+\/(?:LICENSE|[\w-]+\.ttf)))$/.test(
			file,
		)
	return /^(?:licenses\/typst-assets-NOTICE|assets\/(?:(?:linux-x64|macos-arm64)\/(?:typst|LICENSE|NOTICE)|windows-x64\/(?:typst\.exe|LICENSE|NOTICE)))$/.test(
		file,
	)
}

export async function packResources(
	destination = path.join(root, '.resource-packages'),
	records = false,
	repository,
) {
	await mkdir(destination, { recursive: true })
	const archives = []
	for (const kind of ['fonts', 'typst']) {
		const source = path.join(root, kind)
		const manifest = await json(path.join(source, 'package.json'))
		const pins =
			kind === 'typst'
				? await json(path.join(source, 'sources.json'))
				: undefined
		const trustedUpstream = Object.fromEntries(
			Object.entries(pins?.targets ?? {}).map(([target, spec]) => [
				`assets/${target}/${spec.binary}`,
				spec.binarySha256,
			]),
		)
		const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-resource-'))
		try {
			for (const file of manifest.files)
				await cp(path.join(source, file), path.join(temporary, file), {
					recursive: true,
				})
			if (kind === 'typst')
				for (const target of ['linux-x64', 'macos-arm64', 'windows-x64'])
					await rm(path.join(temporary, 'assets', target, 'README.md'), {
						force: true,
					})
			const published = Object.fromEntries(
				[
					'name',
					'version',
					'description',
					'type',
					'license',
					'exports',
					'files',
					'engines',
				]
					.filter((key) => manifest[key] !== undefined)
					.map((key) => [key, manifest[key]]),
			)
			published.homepage = 'https://bluink.app'
			const publicRepository =
				repository ?? manifest.repository?.url?.replace(/\.git$/, '')
			if (publicRepository)
				published.repository = {
					type: 'git',
					url: `${publicRepository}.git`,
					directory: kind,
				}
			published.publishConfig = { access: 'public' }
			await writeJson(path.join(temporary, 'package.json'), published)
			const archive = await packDirectory(temporary, destination, {
				trustedUpstream,
				allowed: (file) => resourceFileAllowed(kind, file),
				required: [
					'LICENSE',
					'README.md',
					'index.js',
					'sources.json',
					...(kind === 'fonts'
						? ['assets/web/inter.css', 'assets/pdf/inter/LICENSE']
						: [
								'assets/linux-x64/typst',
								'assets/macos-arm64/typst',
								'assets/windows-x64/typst.exe',
								'licenses/typst-assets-NOTICE',
							]),
				],
			})
			archives.push(records ? archive : archive.archive)
		} finally {
			await rm(temporary, { recursive: true, force: true })
		}
	}
	return archives
}
