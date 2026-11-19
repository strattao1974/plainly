# Code signing with SignPath (maintainer notes)

Plainly is set up to be signed through the [SignPath Foundation](https://signpath.org)'s free open-source
program. Everything on the repository side is ready. What's left is the application, which needs a person,
and switching signing on afterwards.

## Before applying

- **Reputation.** SignPath won't sign software "based on source code that nobody knows". Apply once Plainly
  has some real users, stars, mentions or downloads.
- **Two-factor sign-in.** SignPath requires multi-factor authentication on both GitHub and SignPath for
  everyone on the team. Check GitHub under Settings > Password and authentication.
- **Already done in this repo:**
  - MIT licence, with no closed-source parts.
  - A public release that includes the installer.
  - An uninstaller.
  - A privacy policy (`PRIVACY.md`) that the installer shows, with installer options to turn off both
    background network features.
  - Version info (product name "Plainly") in both the exe and the installer.
  - The "Code signing policy" section in the README.
  - CI builds on GitHub-hosted runners, with no caches.

## Applying

Apply at <https://signpath.org/apply>. The project is <https://github.com/strattao1974/plainly>, the licence
is MIT, and the files to sign are `plainly.exe` and `PlainlySetup.exe`, built by
`.github/workflows/release.yml`.

## After approval

1. In SignPath, add the predefined Trusted Build System **GitHub.com** to the organization and link it to
   the project.
2. Create two artifact configurations in the project, using these slugs. GitHub uploads arrive as ZIP
   files, so the root element is `<zip-file>`.

   `exe`:
   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file path="plainly.exe" product-name="Plainly">
         <authenticode-sign/>
       </pe-file>
     </zip-file>
   </artifact-configuration>
   ```

   `installer`:
   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file path="PlainlySetup.exe" product-name="Plainly">
         <authenticode-sign/>
       </pe-file>
     </zip-file>
   </artifact-configuration>
   ```
   Check the metadata attributes (`product-name`, and `product-version` if SignPath asks for it) in their
   editor, because their terms require enforcing product name and version.
3. In the GitHub repo (Settings > Secrets and variables > Actions), add:
   - Secret `SIGNPATH_API_TOKEN`: an API token for a SignPath user with *submitter* permission.
   - Variable `SIGNPATH_ORGANIZATION_ID`
   - Variable `SIGNPATH_PROJECT_SLUG`
   - Variable `SIGNPATH_POLICY_SLUG`

   Setting `SIGNPATH_ORGANIZATION_ID` is what switches signing on in the release workflow.
4. In the README's "Code signing policy" section, replace the "pending" paragraph with SignPath's
   attribution line:
   `Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)`

## Releasing (signed or not)

1. Bump `version` in `Cargo.toml`, commit, and push.
2. Tag and push the tag: `git tag v0.2.0` then `git push origin v0.2.0`.
3. The Release workflow tests and builds. When signing is on, it pauses twice for approval in SignPath:
   once for the exe, then for the installer, which is built from the signed exe. Then it creates a **draft**
   release with both files.
4. Review the draft on GitHub, edit the notes, and publish. People running Plainly hear about it within a day.
