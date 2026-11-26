// SPDX-License-Identifier: GPL-2.0-or-later
// Publishes website/public/ at the site root: recommended-feeds.json is the list of feeds the
// app offers a new reader, fetched from https://fleet-reader.app/recommended-feeds.json
// (src/recommended.rs).
export default {
  apiVersion: 1,
  publicDirs: ['./public'],
};
