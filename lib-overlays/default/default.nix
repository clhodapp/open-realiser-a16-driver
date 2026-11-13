# SPDX-License-Identifier: MIT
{ ... }:
{

  overlay = final: prev: {
    open-realiser-a16-driver = prev.open-realiser-a16-driver or { };
  };

}
