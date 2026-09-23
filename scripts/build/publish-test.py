#!/usr/bin/env python3
"""Publish an immutable test package candidate to a configured S3-compatible store."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]


def publish(config_path, package_dir, metadata, assets=()):
    config = json.loads(config_path.read_text())
    endpoint = config['endpoint'].rstrip('/')
    parsed = urlsplit(endpoint)
    if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError('Invalid S3 endpoint')
    bucket = config['bucket']
    if not re.fullmatch(r'[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]', bucket):
        raise ValueError('Invalid bucket')
    spec = importlib.util.spec_from_file_location('native_release', ROOT / 'scripts/build/native-release.py')
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    version = (package_dir / 'VERSION').read_text().strip()
    if version != native.version():
        raise ValueError('Package version differs from checkout')
    rows = []
    files = {}
    for platform in sorted(native.PLATFORMS):
        row_file = package_dir / f'{platform}.tsv'
        if not row_file.exists():
            continue
        row = row_file.read_text().strip().split('\t')
        archive = package_dir / f'zork-{version}-{platform}.tar.gz'
        if row != [platform, archive.name, native.sha256(archive), native.PLATFORMS[platform]]:
            raise ValueError(f'Invalid metadata for {platform}')
        native.verify_archive(archive, version)
        rows.append('\t'.join(row))
        files[archive.name] = archive
    if not rows:
        raise ValueError('No verified native packages')
    for asset in assets:
        if not asset.is_file() or not (asset.suffix in ('.apk', '.zip', '.dmg') or asset.name.endswith('.tar.gz')) or not re.fullmatch(r'[A-Za-z0-9_.-]+', asset.name):
            raise ValueError('Test assets must be existing APK or desktop archive files with portable names')
        if asset.name in files:
            raise ValueError('Duplicate test asset')
        files[asset.name] = asset
    credentials = dict(line.split('=', 1) for line in Path(config['credentials_file']).read_text().splitlines() if line and not line.startswith('#'))
    env = dict(os.environ, AWS_ACCESS_KEY_ID=credentials['MINIO_ROOT_USER'],
               AWS_SECRET_ACCESS_KEY=credentials['MINIO_ROOT_PASSWORD'],
               AWS_DEFAULT_REGION=config.get('region', 'us-east-1'), AWS_EC2_METADATA_DISABLED='true')
    # Never inherit a session token from an unrelated production AWS profile.
    env.pop('AWS_SESSION_TOKEN', None)
    with tempfile.TemporaryDirectory(prefix='zork-test-publish-') as temporary:
        stage = Path(temporary)
        for name, contents in [('VERSION', version + '\n'), ('manifest.tsv', '\n'.join(rows) + '\n')]:
            (stage / name).write_text(contents)
            files[name] = stage / name
        shutil.copyfile(ROOT / 'scripts/install.sh', stage / 'install.sh')
        files['install.sh'] = stage / 'install.sh'
        # Reuse the product installation page, with no public release fallback.
        render = '''import {installPage} from "./src/install.ts";
import {escape} from "./src/page.ts";
installPage(async()=>new Response(null,{status:404})).then(async response=>{
  const policy=escape(response.headers.get("content-security-policy"));
  process.stdout.write((await response.text()).replace('<meta charset="utf-8">',
    '<meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="'+policy+'">'));
});'''
        page = subprocess.check_output(['pnpm', '--dir', str(ROOT / 'deploy/cloudflare'), 'exec', 'tsx', '-e', render], text=True)
        (stage / 'install.html').write_text(page)
        files['install.html'] = stage / 'install.html'
        checksums = ''.join(f'{native.sha256(path)}  {name}\n' for name, path in sorted(files.items()))
        (stage / 'SHA256SUMS').write_text(checksums)
        files['SHA256SUMS'] = stage / 'SHA256SUMS'
        candidate = hashlib.sha256(checksums.encode()).hexdigest()
        base = f'{endpoint}/{bucket}/{candidate}'
        prefix = f's3://{bucket}/{candidate}/download/v{version}'
        for name, path in files.items():
            options = ['--content-type', 'text/html', '--content-disposition', 'inline'] if name == 'install.html' else []
            subprocess.run(['aws', '--endpoint-url', endpoint, 's3', 'cp', str(path), f'{prefix}/{name}', '--only-show-errors', *options], env=env, check=True)
        # Publish metadata only after every object is available. No mutable latest pointer.
        metadata.parent.mkdir(parents=True, exist_ok=True)
        temporary_metadata = metadata.with_suffix('.tmp')
        temporary_metadata.write_text(json.dumps({'base_url': base, 'version': version, 'install_page': f'{base}/download/v{version}/install.html'}, indent=2) + '\n')
        temporary_metadata.replace(metadata)
        print(f'Published test candidate {candidate}: {", ".join(row.split(chr(9))[0] for row in rows)}')
        print(f'Package metadata: {metadata}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True, help='Private endpoint/bucket/credentials_file configuration')
    parser.add_argument('--packages', type=Path, required=True, help='native-release.py stage outputs')
    parser.add_argument('--metadata', type=Path, required=True, help='Write credential-free test-distribution.json')
    parser.add_argument('--asset', type=Path, action='append', default=[], help='Additional test APK or desktop archive')
    args = parser.parse_args()
    publish(args.config, args.packages, args.metadata, args.asset)


if __name__ == '__main__':
    main()
