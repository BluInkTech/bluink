// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { readFile, readdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { json, root, run, sha256 } from './release-utils.mjs'

/** Preserve licence texts for the locked normal dependency graph on all targets. */
export async function rustNotices(destination) {
	const metadata = JSON.parse(
		await run(
			'cargo',
			[
				'metadata',
				'--manifest-path',
				path.join(root, 'backend/Cargo.toml'),
				'--locked',
				'--format-version',
				'1',
			],
			{ capture: true },
		),
	)
	const packages = new Map(metadata.packages.map((entry) => [entry.id, entry]))
	const nodes = new Map(
		metadata.resolve.nodes.map((entry) => [entry.id, entry]),
	)
	const fallback = await json(
		path.join(root, 'scripts/licenses/rust-upstream.json'),
	)
	const seen = new Set()
	const notices = []
	async function visit(id) {
		if (seen.has(id)) return
		seen.add(id)
		const pkg = packages.get(id)
		if (pkg.source) {
			const name = `${pkg.name}@${pkg.version}`
			const extra = fallback[name]
			const licence =
				pkg.name === 'r-efi' ? 'MIT' : (extra?.license ?? pkg.license)
			if (!licence || /(?:AGPL|\bGPL|LGPL|SSPL|UNLICENSED)/.test(licence))
				throw new Error(
					`Review required for Rust dependency ${name}: ${licence}`,
				)
			const directory = path.dirname(pkg.manifest_path)
			const entries = await readdir(directory, { withFileTypes: true })
			const files = entries
				.filter(
					(entry) =>
						entry.isFile() &&
						/^(?:licen[cs]e|copying|notice)(?:[._-]|$)/i.test(entry.name),
				)
				.map((entry) => entry.name)
			if (pkg.license_file && !files.includes(pkg.license_file))
				files.push(pkg.license_file)
			if (pkg.name === 'r-efi') files.push('AUTHORS')
			const texts = await Promise.all(
				files
					.sort()
					.map(
						async (file) =>
							`${file}\n${await readFile(path.join(directory, file), 'utf8')}`,
					),
			)
			for (const source of extra?.files ?? []) {
				const bytes = await readFile(
					path.join(root, 'scripts/licenses', source.file),
				)
				if (sha256(bytes) !== source.sha256)
					throw new Error(`Rust licence checksum mismatch: ${name}`)
				texts.push(`Source: ${source.url}\n${bytes.toString('utf8')}`)
			}
			if (!texts.length)
				throw new Error(`Missing Rust dependency licence text: ${name}`)
			notices.push({
				name,
				text: `${name}\nLicence: ${licence}\nUpstream: ${pkg.repository ?? pkg.source}\nAuthors: ${pkg.authors.join(', ')}\n${texts.join('\n')}\n`,
			})
		}
		for (const dependency of nodes.get(id)?.deps ?? [])
			if (dependency.dep_kinds.some((kind) => kind.kind === null))
				await visit(dependency.pkg)
	}
	await visit(metadata.resolve.root)
	await writeFile(
		destination,
		notices
			.sort((a, b) => a.name.localeCompare(b.name))
			.map((entry) => entry.text)
			.join('\n--------------------\n\n'),
	)
	return notices.length
}
