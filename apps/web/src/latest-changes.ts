export type LatestChange = {
  sha: string;
  title: string;
  url: string;
  committedAt: string;
  pullRequest: number | null;
};

const latestChanges: readonly LatestChange[] = [
  {
    "sha": "29f12c9",
    "title": "Add portable Dockerfile for the web site",
    "url": "https://github.com/joswayski/captures/pull/19",
    "committedAt": "2026-07-22T00:02:08Z",
    "pullRequest": 19
  },
  {
    "sha": "cea342c",
    "title": "Fix/dismiss delete animations",
    "url": "https://github.com/joswayski/captures/pull/18",
    "committedAt": "2026-07-21T23:52:58Z",
    "pullRequest": 18
  },
  {
    "sha": "d8ee11a",
    "title": "Fix draft release creation",
    "url": "https://github.com/joswayski/captures/pull/17",
    "committedAt": "2026-07-20T20:35:23Z",
    "pullRequest": 17
  },
  {
    "sha": "ed869f9",
    "title": "Add local capture history",
    "url": "https://github.com/joswayski/captures/pull/14",
    "committedAt": "2026-07-20T14:47:07Z",
    "pullRequest": 14
  },
  {
    "sha": "6a47780",
    "title": "Automate releases and in-app updates",
    "url": "https://github.com/joswayski/captures/pull/16",
    "committedAt": "2026-07-20T13:28:35Z",
    "pullRequest": 16
  },
  {
    "sha": "281f823",
    "title": "Polish thumbnail dismiss/delete exit effects",
    "url": "https://github.com/joswayski/captures/pull/15",
    "committedAt": "2026-07-20T13:28:17Z",
    "pullRequest": 15
  }
];

export default latestChanges;
