# Enable private contributions on your GitHub profile

This tool commits to a **private** repository. By default, GitHub does **not** show contributions
to private repositories on your profile's contribution graph. You must opt in once.

When enabled, private contributions appear only as anonymous counts (green squares). Visitors
cannot see the repository name, its contents, or commit messages.

## Option A — from your profile page

1. Sign in to GitHub and open your profile: `https://github.com/<your-username>`
   (click your avatar in the top-right corner → **Your profile**).
2. Scroll to the contribution graph.
3. Above the graph, on the right, click the **Contribution settings** dropdown.
4. Select **Private contributions** so it is checked.

## Option B — from account settings

1. Click your avatar in the top-right corner → **Settings**.
2. Stay on the **Public profile** page (the default page).
3. Scroll to the **Contributions & activity** section.
4. Check **Include private contributions on my profile**.
5. Click **Update preferences**.

## Verify

- Open your profile in a private/incognito browser window (logged out) and check that days with
  commits from this tool show as green.
- New commits can take a few minutes to appear on the graph.

## Still not showing up?

A commit only counts if all of the following are true:

- The commit **author email** is linked to your GitHub account. The tool's default,
  `<id>+<login>@users.noreply.github.com`, always is. If you set `GIT_AUTHOR_EMAIL`, make sure it's
  a verified email under **Settings → Emails**.
- The commit is on the repository's **default branch** (the tool always commits there).
- The repository is **not a fork**.

Official docs:
[Showing your private contributions and achievements on your profile](https://docs.github.com/en/account-and-profile/setting-up-and-managing-your-github-profile/managing-contribution-settings-on-your-profile/showing-your-private-contributions-and-achievements-on-your-profile)
