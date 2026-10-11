// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, mkdtemp, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { packDirectory } from './package-artifacts.mjs'
import { json, root, writeJson } from './release-utils.mjs'

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
