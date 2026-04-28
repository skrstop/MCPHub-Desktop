#!/usr/bin/env python3
"""Sync dark mode CSS classes from mcphub-origin to desktop frontend."""
import subprocess
import os
import re

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

files = [
    "frontend/src/components/AddGroupForm.tsx",
    "frontend/src/components/AddServerForm.tsx",
    "frontend/src/components/AddUserForm.tsx",
    "frontend/src/components/ChangePasswordForm.tsx",
    "frontend/src/components/CloudServerCard.tsx",
    "frontend/src/components/CloudServerDetail.tsx",
    "frontend/src/components/EditGroupForm.tsx",
    "frontend/src/components/EditUserForm.tsx",
    "frontend/src/components/GroupCard.tsx",
    "frontend/src/components/GroupImportForm.tsx",
    "frontend/src/components/JSONImportForm.tsx",
    "frontend/src/components/LogViewer.tsx",
    "frontend/src/components/MCPRouterApiKeyError.tsx",
    "frontend/src/components/MarketServerCard.tsx",
    "frontend/src/components/MarketServerDetail.tsx",
    "frontend/src/components/McpbUploadForm.tsx",
    "frontend/src/components/RegistryServerCard.tsx",
    "frontend/src/components/RegistryServerDetail.tsx",
    "frontend/src/components/ServerForm.tsx",
    "frontend/src/components/ServerToolConfig.tsx",
    "frontend/src/components/TemplateExportForm.tsx",
    "frontend/src/components/TemplateImportForm.tsx",
    "frontend/src/components/UserCard.tsx",
    "frontend/src/components/layout/Header.tsx",
    "frontend/src/components/layout/Sidebar.tsx",
    "frontend/src/components/ui/ConfirmDialog.tsx",
    "frontend/src/components/ui/DefaultPasswordWarningModal.tsx",
    "frontend/src/components/ui/DeleteDialog.tsx",
    "frontend/src/components/ui/DynamicForm.tsx",
    "frontend/src/components/ui/LanguageSwitch.tsx",
    "frontend/src/components/ui/MultiSelect.tsx",
    "frontend/src/components/ui/PromptCard.tsx",
    "frontend/src/components/ui/PromptResult.tsx",
    "frontend/src/components/ui/ResourceCard.tsx",
    "frontend/src/components/ui/ThemeSwitch.tsx",
    "frontend/src/components/ui/ToggleGroup.tsx",
    "frontend/src/components/ui/ToolCard.tsx",
    "frontend/src/components/ui/ToolResult.tsx",
    "frontend/src/pages/ActivityPage.tsx",
    "frontend/src/pages/GroupsPage.tsx",
    "frontend/src/pages/LoginPage.tsx",
    "frontend/src/pages/LogsPage.tsx",
    "frontend/src/pages/MarketPage.tsx",
    "frontend/src/pages/PromptsPage.tsx",
    "frontend/src/pages/ResourcesPage.tsx",
    "frontend/src/pages/ServersPage.tsx",
    "frontend/src/pages/UsersPage.tsx",
]


def is_dark_mode_only_change(old_line, new_line):
    """Check if the only difference between old and new line is dark: class additions/modifications."""
    # Remove all dark: prefixed classes from both lines and compare
    old_without_dark = re.sub(r'\s+dark:[^\s"\']+', '', old_line)
    new_without_dark = re.sub(r'\s+dark:[^\s"\']+', '', new_line)
    return old_without_dark.strip() == new_without_dark.strip()


def parse_diff(diff_output):
    """Parse diff output and return list of (old_lines, new_lines) hunks."""
    hunks = []
    lines = diff_output.split('\n')
    i = 0
    while i < len(lines):
        line = lines[i]
        # Match change hunks: Nc, N,Nc, NcN,N, N,NcN
        if re.match(r'^\d+(,\d+)?c\d+(,\d+)?$', line):
            old_lines = []
            new_lines = []
            i += 1
            while i < len(lines) and lines[i].startswith('<'):
                old_lines.append(lines[i][2:])
                i += 1
            if i < len(lines) and lines[i] == '---':
                i += 1
            while i < len(lines) and lines[i].startswith('>'):
                new_lines.append(lines[i][2:])
                i += 1
            hunks.append((old_lines, new_lines))
        else:
            i += 1
    return hunks


def apply_hunk(content, old_lines, new_lines):
    """Apply a hunk of changes to file content."""
    old_text = '\n'.join(old_lines)
    new_text = '\n'.join(new_lines)
    if old_text in content:
        return content.replace(old_text, new_text, 1), True
    return content, False


total_applied = 0
complex_files = []

for rel_path in files:
    desktop_path = os.path.join(BASE_DIR, rel_path)
    origin_path = os.path.join(BASE_DIR, 'mcphub-origin', rel_path)

    if not os.path.exists(desktop_path) or not os.path.exists(origin_path):
        print(f"MISSING: {rel_path}")
        continue

    result = subprocess.run(['diff', desktop_path, origin_path], capture_output=True, text=True)
    if result.returncode == 0:
        continue  # No difference

    hunks = parse_diff(result.stdout)

    if not hunks:
        complex_files.append(rel_path)
        continue

    # Check all hunks are dark-mode-only changes
    all_dark_only = True
    for old_lines, new_lines in hunks:
        if len(old_lines) != len(new_lines):
            all_dark_only = False
            break
        for old, new in zip(old_lines, new_lines):
            if not is_dark_mode_only_change(old, new):
                all_dark_only = False
                break
        if not all_dark_only:
            break

    if all_dark_only:
        with open(desktop_path, 'r') as f:
            content = f.read()

        modified = content
        applied = 0
        for old_lines, new_lines in hunks:
            modified, ok = apply_hunk(modified, old_lines, new_lines)
            if ok:
                applied += 1
            else:
                print(f"  WARN: Could not find hunk in {rel_path}:")
                print(f"    OLD: {old_lines[0][:80]!r}")

        if applied > 0:
            with open(desktop_path, 'w') as f:
                f.write(modified)
            print(f"  APPLIED {applied} dark mode hunks: {rel_path}")
            total_applied += applied
    else:
        complex_files.append(rel_path)
        for old_lines, new_lines in hunks:
            if len(old_lines) == len(new_lines):
                for old, new in zip(old_lines, new_lines):
                    if not is_dark_mode_only_change(old, new):
                        print(f"  COMPLEX: {rel_path}")
                        print(f"    OLD: {old[:80]!r}")
                        print(f"    NEW: {new[:80]!r}")
                        break
            else:
                print(f"  COMPLEX (line count diff): {rel_path}")
                print(f"    OLD ({len(old_lines)} lines): {old_lines[0][:60]!r}")
                print(f"    NEW ({len(new_lines)} lines): {new_lines[0][:60]!r}")
            break

print(f"\nTotal dark mode hunks applied: {total_applied}")
print(f"\nComplex files needing manual review ({len(complex_files)}):")
for f in complex_files:
    print(f"  - {f}")
