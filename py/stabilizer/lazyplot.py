from matplotlib import pyplot as plt
import csv
import argparse
import numpy as np



def main():
    parser = argparse.ArgumentParser(description="Record raw Stabilizer data")
    parser.add_argument(
        "--filename", type=str, default="data.csv", help="File to safe the data in."
    )
    args = parser.parse_args()
    f = open(args.filename, "r")
    reader = csv.reader(f)
    data = []
    for i, row in enumerate(reader):
        data.append(float(row[0]))
    plt.xlabel("Frequency (Hz)")
    plt.ylabel("Power (dB)")
    plt.title("Transfer Function")
    plt.grid()
    f = np.linspace(10, 20000, len(data))
    plt.plot(f, data)
    plt.show()


if __name__ == "__main__":
    main()
