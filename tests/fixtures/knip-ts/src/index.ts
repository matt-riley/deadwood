import { used } from "./used";
import { E, N } from "./types";
import type { UsedType } from "./types";
import missing from "missing";
import optional from "optional-pkg";
import "./missing";
import "used-pkg";

export type PublicType = UsedType;
console.log(used, E.A, N.used, missing, optional);
