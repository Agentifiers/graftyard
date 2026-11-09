import type { Schemas } from "@/api/client";
import { API, HOST } from "../world";

// What GET /gitignore/templates and GET /licenses serve: a short catalog of each.

export const gitignores: Schemas["gitignore-template"][] = [
  { name: "Go", source: "# Binaries\n*.exe\n*.dll\n*.so\n*.dylib\n*.test\n*.out\ngo.work\n" },
  { name: "Node", source: "node_modules/\nnpm-debug.log*\n.env\ndist/\ncoverage/\n" },
  { name: "Python", source: "__pycache__/\n*.py[cod]\n.venv/\nbuild/\ndist/\n*.egg-info/\n" },
  { name: "Rust", source: "/target/\nCargo.lock.orig\n**/*.rs.bk\n*.pdb\n" },
  {
    name: "Swift",
    source: ".build/\nxcuserdata/\nDerivedData/\n*.xcodeproj/project.xcworkspace/\n",
  },
  { name: "Terraform", source: "**/.terraform/*\n*.tfstate\n*.tfstate.*\ncrash.log\n*.tfvars\n" },
  { name: "TeX", source: "*.aux\n*.log\n*.out\n*.toc\n*.synctex.gz\n" },
  { name: "Unity", source: "/[Ll]ibrary/\n/[Tt]emp/\n/[Oo]bj/\n/[Bb]uild/\n" },
];

function license(
  key: string,
  name: string,
  spdx: string,
  node: string,
  featured: boolean,
): Schemas["license-simple"] & { featured: boolean } {
  return {
    key,
    name,
    spdx_id: spdx,
    url: `${API}/licenses/${key}`,
    node_id: node,
    html_url: `${HOST}/licenses/${key}`,
    featured,
  };
}

export const licenses = [
  license("apache-2.0", "Apache License 2.0", "Apache-2.0", "MDc6TGljZW5zZTI=", true),
  license("mit", "MIT License", "MIT", "MDc6TGljZW5zZTEz", true),
  license("gpl-3.0", "GNU General Public License v3.0", "GPL-3.0", "MDc6TGljZW5zZTk=", true),
  license(
    "bsd-3-clause",
    'BSD 3-Clause "New" or "Revised" License',
    "BSD-3-Clause",
    "MDc6TGljZW5zZTU=",
    false,
  ),
  license("mpl-2.0", "Mozilla Public License 2.0", "MPL-2.0", "MDc6TGljZW5zZTE0", false),
  license(
    "agpl-3.0",
    "GNU Affero General Public License v3.0",
    "AGPL-3.0",
    "MDc6TGljZW5zZTA=",
    false,
  ),
  license("unlicense", "The Unlicense", "Unlicense", "MDc6TGljZW5zZTE1", false),
];
