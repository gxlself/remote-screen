import { useIdentity } from '../data/queries';

/**
 * Creates the device identity at launch. Pairing, SSH keys and host sessions
 * all sign with it; the Hub sign-in used to create it and is gone.
 */
export default function IdentityBootstrap(): null {
  useIdentity();
  return null;
}
