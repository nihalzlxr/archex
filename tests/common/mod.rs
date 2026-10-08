#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Build a minimal Next.js-shaped project inside `root` so that
/// `archex init` detects Next.js, seeds modules, and indexes symbols/imports.
/// Content is kept JSX-free so the TypeScript grammar parses it cleanly.
pub fn fixture_project(root: &Path) {
    fs::write(
        root.join("package.json"),
        r#"{ "dependencies": { "next": "14.0.0" } }"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::write(
        root.join("src/components/Button.tsx"),
        "export function Button(label: string): string {\n  return label;\n}\n",
    )
    .unwrap();

    fs::write(
        root.join("src/components/Client.tsx"),
        "export function loadUser() {\n  fetch('/api/user');\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/db")).unwrap();
    fs::write(
        root.join("src/db/client.ts"),
        "export function db() {\n  return null;\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/app/api/auth")).unwrap();
    fs::write(
        root.join("src/app/api/auth/route.ts"),
        "import { db } from '@/db/client';\nexport function GET(): Response {\n  return Response.json({ ok: true });\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/app")).unwrap();
    fs::write(
        root.join("src/app/page.tsx"),
        "import { Button } from '@/components/Button';\nexport default function Page() {\n  return Button('hi');\n}\n",
    )
    .unwrap();

    fs::write(
        root.join("src/app/dashboard.tsx"),
        "export function Dashboard() {\n  fetch('/api/data');\n  return null;\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/lib")).unwrap();
    fs::write(
        root.join("src/lib/utils.ts"),
        "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/services")).unwrap();
    fs::write(
        root.join("src/services/userService.ts"),
        "import { db } from '@/db/client';\nexport function getUser(id: string) {\n  return db();\n}\n",
    )
    .unwrap();
}
