//! Shared diff fixtures for the analysis tests (included by the other `analysis_*.rs` files via
//! `#[path]`; compiled on its own it just checks the fixtures parse).
#![allow(dead_code)]

/// A realistic `git diff base...head` for the Frappe MR from docs/plan.html, plus a new test file
/// with a token, a deleted legacy module (whose f-string SQL must NOT be flagged), a rename with an
/// edit, a pure rename, a mode change, binary files, a yarn.lock change and a TSX file.
pub const MR_DIFF: &str = "\
diff --git a/entries_crm/api/import_links.py b/entries_crm/api/import_links.py
index 3b18e51..a9c2f04 100644
--- a/entries_crm/api/import_links.py
+++ b/entries_crm/api/import_links.py
@@ -8,7 +8,14 @@ import json
 
 import frappe
 
-# legacy linker removed
+
+@frappe.whitelist()
+def link_import_rows(import_name, rows):
+    rows = json.loads(rows)
+    for r in rows:
+        frappe.db.sql(f\"update `tabECRM Lead` set import_ref='{import_name}' where name='{r}'\")
+    frappe.db.commit()
+
 
 def _helper():
     pass
@@ -40,3 +47,4 @@ def _other():
     x = 1
     y = 2
-    return x
+    # sum both values
+    return x + y
diff --git a/entries_crm/api/test_import_links.py b/entries_crm/api/test_import_links.py
new file mode 100644
index 0000000..e69c3d1
--- /dev/null
+++ b/entries_crm/api/test_import_links.py
@@ -0,0 +1,4 @@
+import frappe
+
+GITLAB_TOKEN = \"glpat-AbCdEf1234567890xYz_q\"
+password = \"changeme\"
diff --git a/entries_crm/legacy/linker.py b/entries_crm/legacy/linker.py
deleted file mode 100644
index 5d1e2aa..0000000
--- a/entries_crm/legacy/linker.py
+++ /dev/null
@@ -1,3 +0,0 @@
-def link(name):
-    frappe.db.sql(f\"select * from tabLead where name='{name}'\")
-    AWS_KEY = \"AKIAZ3MQRSTUVWXY1234\"
diff --git a/entries_crm/utils/old_name.py b/entries_crm/utils/new_name.py
similarity index 90%
rename from entries_crm/utils/old_name.py
rename to entries_crm/utils/new_name.py
index 1111111..2222222 100644
--- a/entries_crm/utils/old_name.py
+++ b/entries_crm/utils/new_name.py
@@ -1,3 +1,3 @@
 def helper():
-    return 1
+    return 2
 # end
\\ No newline at end of file
diff --git a/docs/a.md b/docs/b.md
similarity index 100%
rename from docs/a.md
rename to docs/b.md
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/ui/public/logo.png b/ui/public/logo.png
index 3c4f001..9a8b002 100644
Binary files a/ui/public/logo.png and b/ui/public/logo.png differ
diff --git a/ui/public/new.png b/ui/public/new.png
new file mode 100644
index 0000000..9a8b003
Binary files /dev/null and b/ui/public/new.png differ
diff --git a/yarn.lock b/yarn.lock
index aaaaaaa..bbbbbbb 100644
--- a/yarn.lock
+++ b/yarn.lock
@@ -1 +1 @@
-lodash@4.17.20
+lodash@4.17.21
diff --git a/ui/src/ReviewTable.tsx b/ui/src/ReviewTable.tsx
index ccccccc..ddddddd 100644
--- a/ui/src/ReviewTable.tsx
+++ b/ui/src/ReviewTable.tsx
@@ -86,4 +86,5 @@ export function ReviewTable() {
   const [page, setPage] = useState(0);
-  const rows = data;
+  const rows = data.slice(page * 20, page * 20 + 20);
+  // frappe.db.sql(f\"x\") in TS must not trigger the python-only rule
   return <Table rows={rows} />;
 }
";

pub const IMPORT_LINKS: &str = "entries_crm/api/import_links.py";
pub const TEST_FILE: &str = "entries_crm/api/test_import_links.py";
pub const LEGACY: &str = "entries_crm/legacy/linker.py";
pub const OLD_NAME: &str = "entries_crm/utils/old_name.py";
pub const NEW_NAME: &str = "entries_crm/utils/new_name.py";
pub const TSX: &str = "ui/src/ReviewTable.tsx";

pub const SHAS: (&str, &str, &str) = ("base000", "start111", "head222");

#[test]
fn fixture_parses() {
    let files = warden_core::diff::parse_unified(MR_DIFF);
    assert_eq!(files.len(), 10);
}
