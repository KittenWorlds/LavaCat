#!/usr/bin/env python3
"""Inject reproducible NOTICE / multiple-license tables after cargo-about.
Usage: python tools/licenses/inject-notes.py about.json dependencies.html
Both inputs come from the same locked Cargo graph; no network access is used.
"""
import html
import json
import re
import sys
from collections import Counter
from pathlib import Path

report = json.loads(Path(sys.argv[1]).read_text())
target = Path(sys.argv[2])
escape = lambda value: html.escape(str(value), quote=True)
packages = {entry['package']['id']: entry['package'] for entry in report['crates']}
counts = Counter(used['crate']['id'] for group in report['licenses'] for used in group['used_by'])
notices = []
for package in sorted(packages.values(), key=lambda p: (p['name'], p['version'])):
    root = Path(package['manifest_path']).parent
    for path in sorted(root.iterdir()):
        if path.is_file() and re.match(r'^(NOTICE|NOTICES|COPYING|AUTHORS)([.\-_]|$)', path.name, re.I):
            text = path.read_text(errors='replace')
            notices.append(f'<tr><td>{escape(package["name"])}</td><td>{escape(package["version"])}</td><td>{escape(path.name)}</td><td><details><summary>展开</summary><pre>{escape(text)}</pre></details></td></tr>')
notice_html = f'<p class="note">共 {len(notices)} 份随包分发的归属声明文件。</p><table><thead><tr><th>crate</th><th>版本</th><th>声明文件</th><th>正文</th></tr></thead><tbody>' + ''.join(notices) + '</tbody></table>'
multiple = []
for ident, count in sorted(counts.items()):
    package = packages[ident]
    expression = package.get('license') or 'UNKNOWN'
    if count > 1 or ' AND ' in expression:
        multiple.append(f'<tr class="multi"><td>{escape(package["name"])}</td><td>{escape(package["version"])}</td><td class="spdx">{escape(expression)}</td><td>{count}</td></tr>')
multi_html = '<h2>多重许可核对表</h2><p class="note">AND 表示义务叠加；OR 为可选许可路径。表格列出跨组或含 AND 的依赖，正文见上方许可章节。</p><table><thead><tr><th>crate</th><th>版本</th><th>SPDX</th><th>章节数</th></tr></thead><tbody>' + ''.join(multiple) + '</tbody></table>'
result = target.read_text()
for key, block in [('NOTICES', notice_html), ('MULTI', multi_html)]:
    result, count = re.subn(f'(<!--CARGO-ABOUT:{key}-->).*?(<!--/CARGO-ABOUT:{key}-->)', lambda match: match[1] + '\n' + block + '\n' + match[2], result, flags=re.S)
    if count != 1:
        raise SystemExit(f'expected one {key} placeholder, found {count}')
target.write_text("\n".join(line.rstrip() for line in result.splitlines()) + "\n")
print(f'Injected {len(notices)} notice files and {len(multiple)} multiple-license rows')
